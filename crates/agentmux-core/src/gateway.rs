//! The public-facing side of AgentMux, for agents that run in someone's cloud (Grok routines).
//!
//! * a token-guarded streamable-HTTP MCP endpoint on 127.0.0.1 (`POST /mcp`), and
//! * an optional tunnel (cloudflared by default, or ngrok) that publishes that endpoint over HTTPS.
//!
//! Nothing here is reachable from outside until the user starts a tunnel, and even then the token
//! only unlocks a small tool set (`REMOTE_TOOLS`): enough to answer a question, never to wake other
//! agents or read the workspace.

use crate::core::{CallerCtx, Core};
use crate::{config, mcp, secrets, wakers};
use anyhow::{anyhow, bail, Result};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{oneshot, Notify};

/// The original bot (`grok`) keeps the pre-multi-bot secret name.
fn token_secret(agent: &str) -> String {
    match agent.strip_prefix("grok:") {
        Some(slug) => format!("grok-mcp-token:{slug}"),
        None => "grok-mcp-token".into(),
    }
}
/// Tools a remote (tunnelled) caller may use.
pub const REMOTE_TOOLS: &[&str] = &["mux_whoami", "mux_contacts", "mux_register", "mux_inbox", "mux_reply", "mux_updates"];
/// Added for a bot that has been switched on to start conversations.
pub const ASK_TOOLS: &[&str] = &["mux_ask_peer", "mux_get_reply"];
/// Per bot: at most this many new questions to other agents per hour.
const MAX_ASKS_PER_HOUR: usize = 30;
const CF_TOKEN_SECRET: &str = "cloudflare-tunnel-token";
const NGROK_TOKEN_SECRET: &str = "ngrok-authtoken";
pub const PROVIDERS: &[&str] = &["cloudflared", "ngrok"];
const DEFAULT_PORT: u16 = 4317;
const START_TIMEOUT: Duration = Duration::from_secs(45);

fn metrics_port(port: u16) -> u16 {
    port.wrapping_add(20000)
}

/// First pid whose command line matches `pattern` (pgrep -f).
fn find_pid(pattern: &str) -> Option<u32> {
    let out = std::process::Command::new("pgrep").args(["-f", pattern]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.trim().parse().ok())
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn port() -> u16 {
    std::env::var("AGENTMUX_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Stopped,
    Starting,
    Running,
    Failed,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Stopped => "stopped",
            Phase::Starting => "starting",
            Phase::Running => "running",
            Phase::Failed => "error",
        }
    }
}

struct TunnelState {
    phase: Phase,
    url: Option<String>,
    error: Option<String>,
    last_log: Option<String>,
    kill: Option<Arc<Notify>>,
    done: Option<oneshot::Receiver<()>>,
    /// Bumped on every start/stop so a stale supervisor can't overwrite newer state.
    epoch: u64,
}

struct Limiter {
    window: Instant,
    total: u32,
    bad: u32,
}

pub struct Gateway {
    core: Arc<Core>,
    tokens: Mutex<HashMap<String, String>>,
    tunnel: Mutex<TunnelState>,
    http: Mutex<Option<String>>,
    limiter: Mutex<Limiter>,
    asks: Mutex<HashMap<String, Vec<Instant>>>,
    paused: std::sync::atomic::AtomicBool,
}

impl Gateway {
    pub fn new(core: Arc<Core>) -> Arc<Self> {
        Arc::new(Self {
            core,
            tokens: Mutex::new(HashMap::new()),
            tunnel: Mutex::new(TunnelState { phase: Phase::Stopped, url: None, error: None, last_log: None, kill: None, done: None, epoch: 0 }),
            http: Mutex::new(None),
            limiter: Mutex::new(Limiter { window: Instant::now(), total: 0, bad: 0 }),
            asks: Mutex::new(HashMap::new()),
            paused: std::sync::atomic::AtomicBool::new(false),
        })
    }

    // ───────── token ─────────

    /// The bearer token one Grok bot presents. Created on first use and kept in the OS keychain.
    pub fn token(&self, agent: &str) -> Result<String> {
        if !crate::providers::remote_access(agent) {
            bail!("'{agent}' does not accept remote connections");
        }
        if let Some(t) = self.tokens.lock().unwrap().get(agent) {
            return Ok(t.clone());
        }
        let t = match secrets::get(&token_secret(agent))? {
            Some(t) => t,
            None => {
                let t = new_token();
                secrets::set(&token_secret(agent), &t)?;
                t
            }
        };
        self.tokens.lock().unwrap().insert(agent.to_string(), t.clone());
        Ok(t)
    }

    /// Replace a bot's token; anything holding the old one is locked out immediately.
    pub fn rotate_token(&self, agent: &str) -> Result<String> {
        if !crate::providers::remote_access(agent) {
            bail!("'{agent}' does not accept remote connections");
        }
        let t = new_token();
        secrets::set(&token_secret(agent), &t)?;
        self.tokens.lock().unwrap().insert(agent.to_string(), t.clone());
        Ok(t)
    }

    /// Forget every cached token; the Keychain is read again on the next request.
    pub fn reload_tokens(&self) {
        self.tokens.lock().unwrap().clear();
    }

    /// Drop a removed bot's token.
    pub fn forget(&self, agent: &str) {
        let _ = secrets::delete(&token_secret(agent));
        self.tokens.lock().unwrap().remove(agent);
    }

    /// Which bot owns this bearer token? Existing tokens only; nothing is created here. If nothing
    /// matches, the keychain is read again once, so a token rotated by another process (the CLI)
    /// takes effect without a restart.
    fn identify(&self, presented: &str) -> Option<String> {
        if let Some(a) = self.identify_once(presented) {
            return Some(a);
        }
        self.tokens.lock().unwrap().clear();
        self.identify_once(presented)
    }

    fn identify_once(&self, presented: &str) -> Option<String> {
        let bots: Vec<String> = self
            .core
            .vault
            .providers()
            .ok()?
            .into_iter()
            .filter(|p| p["family"] == "grok")
            .filter_map(|p| p["id"].as_str().map(String::from))
            .collect();
        let mut found = None;
        for agent in bots {
            let cached = self.tokens.lock().unwrap().get(&agent).cloned();
            let token = match cached {
                Some(t) => Some(t),
                None => secrets::get(&token_secret(&agent)).ok().flatten().inspect(|t| {
                    self.tokens.lock().unwrap().insert(agent.clone(), t.clone());
                }),
            };
            // Compare against every bot so timing doesn't reveal which one matched.
            if let Some(t) = token {
                if ct_eq(presented.as_bytes(), t.as_bytes()) && found.is_none() {
                    found = Some(agent);
                }
            }
        }
        found
    }

    /// While paused (AgentMux turned off) the HTTP endpoint answers 503 and the tunnel is down.
    pub fn set_paused(&self, on: bool) {
        self.paused.store(on, std::sync::atomic::Ordering::SeqCst);
        self.emit();
    }

    /// Whether the HTTP endpoint is listening (it may wait for another AgentMux process to free the port).
    pub fn http_bound(&self) -> bool {
        self.http.lock().unwrap().is_some()
    }

    pub fn paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::SeqCst)
    }

    // ───────── settings ─────────

    pub fn provider(&self) -> String {
        self.core
            .vault
            .setting("tunnel.provider")
            .ok()
            .flatten()
            .filter(|p| PROVIDERS.contains(&p.as_str()))
            .unwrap_or_else(|| "cloudflared".into())
    }

    fn ngrok_domain(&self) -> Option<String> {
        self.setting("tunnel.ngrok_domain")
    }

    fn setting(&self, key: &str) -> Option<String> {
        self.core.vault.setting(key).ok().flatten().filter(|v| !v.is_empty())
    }

    /// `quick` (temporary trycloudflare URL), `named` (tunnel in the user's account, created here)
    /// or `token` (tunnel made in the Cloudflare dashboard).
    fn cf_mode(&self) -> String {
        self.setting("tunnel.cf_mode").filter(|m| matches!(m.as_str(), "named" | "token")).unwrap_or_else(|| "quick".into())
    }

    /// A tunnel whose public URL survives restarts.
    pub fn permanent(&self) -> bool {
        match self.provider().as_str() {
            "ngrok" => self.ngrok_domain().is_some(),
            _ => self.cf_mode() != "quick" && self.setting("tunnel.cf_hostname").is_some(),
        }
    }

    fn permanent_host(&self) -> Option<String> {
        if !self.permanent() {
            return None;
        }
        if self.provider() == "ngrok" { self.ngrok_domain() } else { self.setting("tunnel.cf_hostname") }
    }

    pub fn autostart(&self) -> bool {
        self.setting("tunnel.autostart").as_deref() == Some("1")
    }

    pub fn set_autostart(&self, on: bool) -> Result<()> {
        if on && !self.permanent() {
            bail!("only a permanent tunnel can start automatically; a temporary URL would change every launch");
        }
        self.core.vault.set_setting("tunnel.autostart", if on { "1" } else { "0" })?;
        self.emit();
        Ok(())
    }

/// Drop a stale "tunnel exited" message once the user changes the setup.
    fn clear_error(&self) {
        let mut t = self.tunnel.lock().unwrap();
        if !matches!(t.phase, Phase::Starting | Phase::Running) {
            t.error = None;
            t.phase = Phase::Stopped;
        }
    }

        fn ensure_stopped(&self) -> Result<()> {
        if matches!(self.tunnel.lock().unwrap().phase, Phase::Starting | Phase::Running) {
            bail!("stop the tunnel first");
        }
        Ok(())
    }

    // ───────── permanent tunnels: Cloudflare ─────────

    fn cf_cert() -> std::path::PathBuf {
        std::env::var("TUNNEL_ORIGIN_CERT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".cloudflared/cert.pem"))
    }

    /// Runs cloudflared; returns (stdout, stdout+stderr). Its log lines go to stderr.
    async fn cloudflared(args: &[&str]) -> Result<(String, String)> {
        let bin = wakers::find_bin("cloudflared").ok_or_else(|| anyhow!("cloudflared is not installed. Install it with: brew install cloudflared"))?;
        let out = Command::new(bin).args(args).env("PATH", wakers::spawn_path()).stdin(std::process::Stdio::null()).output().await?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() {
            let tail: String = text.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            bail!("cloudflared {} failed: {}", args.first().copied().unwrap_or(""), tail.trim());
        }
        Ok((String::from_utf8_lossy(&out.stdout).to_string(), text))
    }

    /// The domain the Cloudflare login was authorised for. The login certificate only carries the
    /// zone id, so ask the API with the certificate's own (tunnel-scoped) token.
    pub async fn cf_detect_zone(&self) -> Result<String> {
        use base64::Engine;
        let pem = std::fs::read_to_string(Self::cf_cert())?;
        let b64: String = pem
            .split("-----BEGIN ARGO TUNNEL TOKEN-----")
            .nth(1)
            .and_then(|r| r.split("-----END ARGO TUNNEL TOKEN-----").next())
            .ok_or_else(|| anyhow!("the Cloudflare login file has no tunnel token"))?
            .split_whitespace()
            .collect();
        let tok: Value = serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(b64)?)?;
        let (zone_id, account, api) = (
            tok["zoneID"].as_str().unwrap_or(""),
            tok["accountID"].as_str().unwrap_or(""),
            tok["apiToken"].as_str().unwrap_or(""),
        );
        let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
        let v: Value = client
            .get(format!("https://api.cloudflare.com/client/v4/zones?account.id={account}&per_page=50"))
            .bearer_auth(api)
            .send()
            .await?
            .json()
            .await?;
        let name = v["result"]
            .as_array()
            .and_then(|zs| zs.iter().find(|z| z["id"] == zone_id))
            .and_then(|z| z["name"].as_str())
            .ok_or_else(|| anyhow!("Cloudflare didn't return the authorised domain"))?
            .to_string();
        self.core.vault.set_setting("tunnel.cf_zone", &name)?;
        self.emit();
        Ok(name)
    }

    /// Opens the browser so the user can authorise a domain. Waits (up to 5 min) for the cert,
    /// then fills in which domain was picked.
    pub async fn cf_login(&self) -> Result<()> {
        if Self::cf_cert().exists() {
            if self.setting("tunnel.cf_zone").is_none() {
                let _ = self.cf_detect_zone().await;
            }
            return Ok(());
        }
        tokio::time::timeout(Duration::from_secs(300), Self::cloudflared(&["tunnel", "login"]))
            .await
            .map_err(|_| anyhow!("timed out waiting for the Cloudflare login in your browser"))??;
        if !Self::cf_cert().exists() {
            bail!("Cloudflare login finished but no certificate was saved");
        }
        if let Err(e) = self.cf_detect_zone().await {
            tracing::warn!("could not read the authorised Cloudflare domain: {e}");
        }
        self.emit();
        Ok(())
    }

    /// Log in again and pick a different domain. The old login is kept as a backup file.
    pub async fn cf_switch_domain(&self) -> Result<()> {
        let cert = Self::cf_cert();
        if cert.exists() {
            let bak = cert.with_file_name(format!("cert.pem.agentmux-{}.bak", chrono::Utc::now().format("%Y%m%d%H%M%S")));
            std::fs::rename(&cert, &bak)?;
        }
        let _ = self.core.vault.del_setting("tunnel.cf_zone");
        self.clear_error();
        self.emit();
        self.cf_login().await
    }

    /// Create (or reuse) a named tunnel in the user's account and point a hostname at it.
    /// `input` is a full hostname (`mux.example.com`) or just a subdomain (`mux`): Cloudflare
    /// then puts it under the domain picked in the browser at login, and we read back which.
    pub async fn cf_create(&self, input: &str) -> Result<String> {
        let raw = input.trim().trim_end_matches('.').to_lowercase();
        let label_only = !raw.contains('.');
        if label_only {
            if raw.is_empty() || crate::config::slug(&raw) != raw {
                bail!("enter a subdomain like mux (letters, numbers and dashes), or a full hostname");
            }
        } else {
            valid_hostname(&raw)?;
        }
        if !Self::cf_cert().exists() {
            bail!("connect Cloudflare first");
        }
        let name = format!("agentmux-{}", crate::config::slug(&raw));
        let find = |json: &str| -> Option<String> {
            let v: Value = serde_json::from_str(json).ok()?;
            v.as_array()?.iter().find(|t| t["deleted_at"].is_null() || t["deleted_at"].as_str().is_some_and(|d| d.starts_with("0001")))?["id"]
                .as_str()
                .map(String::from)
        };
        let mut id = find(&Self::cloudflared(&["tunnel", "list", "-o", "json", "-n", &name]).await?.0);
        if id.is_none() {
            Self::cloudflared(&["tunnel", "create", &name]).await?;
            id = find(&Self::cloudflared(&["tunnel", "list", "-o", "json", "-n", &name]).await?.0);
        }
        let id = id.ok_or_else(|| anyhow!("created tunnel {name} but could not read its id"))?;
        let (_, log) = Self::cloudflared(&["tunnel", "route", "dns", "--overwrite-dns", &id, &raw]).await?;
        let host = routed_hostname(&log).or_else(|| (!label_only).then(|| raw.clone())).ok_or_else(|| {
            anyhow!("Cloudflare didn't say which domain it used. Enter the full hostname instead, e.g. {raw}.yourdomain.com")
        })?;
        let host = valid_hostname(&host)?;
        if !label_only && host != raw {
            // cloudflared puts a name outside the logged-in zone *under* that zone
            // (mux.other.com -> mux.other.com.yourzone.com). Never report the wrong URL.
            let zone = host.strip_prefix(&format!("{raw}.")).unwrap_or("the domain you connected").to_string();
            if zone.contains('.') {
                let _ = self.core.vault.set_setting("tunnel.cf_zone", &zone);
            }
            self.emit();
            bail!(
                "{raw} isn't on the Cloudflare domain you connected ({zone}), so Cloudflare created {host} instead. \
Delete that DNS record in the Cloudflare dashboard, then use a subdomain of {zone}, or connect the domain you want with \"use a different domain\" (its DNS must be on Cloudflare)."
            );
        }
        let v = &self.core.vault;
        v.set_setting("tunnel.provider", "cloudflared")?;
        v.del_setting("tunnel.verified_host")?;
        v.set_setting("tunnel.cf_mode", "named")?;
        v.set_setting("tunnel.cf_tunnel", &id)?;
        v.set_setting("tunnel.cf_hostname", &host)?;
        if let Some((_, zone)) = host.split_once('.') {
            if label_only {
                v.set_setting("tunnel.cf_zone", zone)?;
            }
        }
        self.emit();
        Ok(host)
    }

    /// Check the public URL really reaches this app (DNS can take a little while to appear).
    pub async fn verify(&self) -> Result<String> {
        let url = self.tunnel.lock().unwrap().url.clone().ok_or_else(|| anyhow!("the tunnel isn't running"))?;
        let host = url.trim_start_matches("https://").to_string();
        if tokio::net::lookup_host((host.as_str(), 443)).await.is_err() {
            bail!(
                "{host} doesn't resolve. Its domain's DNS must be on Cloudflare (check the domain's nameservers), and the record may take a few minutes to appear."
            );
        }
        let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build()?;
        let mut last = String::new();
        for _ in 0..12 {
            match client.get(format!("{url}/healthz")).send().await {
                Ok(r) if r.status().is_success() => {
                    let v: Value = r.json().await.unwrap_or(Value::Null);
                    if v["service"] == "agentmux" {
                        self.core.vault.set_setting("tunnel.verified_host", &host)?;
                        self.emit();
                        return Ok(format!("Verified: {url}/mcp reaches this Mac from the internet"));
                    }
                    last = "something else answered at that address".into();
                }
                Ok(r) => last = format!("HTTP {}", r.status().as_u16()),
                Err(e) => last = e.without_url().to_string(),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        bail!("{url} isn't reachable yet ({last}). New DNS records can take a few minutes; try Test again shortly.")
    }

    /// Use a tunnel created in the Cloudflare dashboard. Its public hostname must point at
    /// http://localhost:<port> there; the token goes to the keychain.
    pub fn cf_use_token(&self, token: &str, hostname: &str) -> Result<()> {
        self.ensure_stopped()?;
        let host = valid_hostname(hostname)?;
        let token = token.trim();
        if token.len() < 40 || token.contains(char::is_whitespace) {
            bail!("that doesn't look like a Cloudflare tunnel token (copy it from the tunnel's install command)");
        }
        secrets::set(CF_TOKEN_SECRET, token)?;
        let v = &self.core.vault;
        v.set_setting("tunnel.provider", "cloudflared")?;
        v.set_setting("tunnel.cf_mode", "token")?;
        v.set_setting("tunnel.cf_hostname", &host)?;
        self.emit();
        Ok(())
    }

    /// Back to temporary quick tunnels. The tunnel and DNS record stay in the user's account.
    pub fn cf_reset(&self) -> Result<()> {
        self.ensure_stopped()?;
        let v = &self.core.vault;
        for k in ["tunnel.cf_mode", "tunnel.cf_tunnel", "tunnel.cf_hostname", "tunnel.cf_zone", "tunnel.verified_host"] {
            v.del_setting(k)?;
        }
        let _ = secrets::delete(CF_TOKEN_SECRET);
        v.set_setting("tunnel.autostart", "0")?;
        self.clear_error();
        self.emit();
        Ok(())
    }

    // ───────── permanent tunnels: ngrok ─────────

    /// Store the ngrok authtoken (keychain) and/or the reserved domain.
    pub fn ngrok_setup(&self, authtoken: Option<&str>, domain: Option<&str>) -> Result<()> {
        self.ensure_stopped()?;
        if let Some(t) = authtoken.map(str::trim).filter(|t| !t.is_empty()) {
            if t.contains(char::is_whitespace) {
                bail!("the ngrok authtoken can't contain spaces");
            }
            secrets::set(NGROK_TOKEN_SECRET, t)?;
            self.core.vault.set_setting("tunnel.ngrok_authtoken", "1")?;
        }
        self.core.vault.set_setting("tunnel.provider", "ngrok")?;
        self.set_provider("ngrok", domain)
    }

    pub fn set_provider(&self, provider: &str, ngrok_domain: Option<&str>) -> Result<()> {
        if !PROVIDERS.contains(&provider) {
            bail!("unknown tunnel provider '{provider}' (use cloudflared or ngrok)");
        }
        if matches!(self.tunnel.lock().unwrap().phase, Phase::Starting | Phase::Running) {
            bail!("stop the tunnel before changing the provider");
        }
        let domain = ngrok_domain.map(str::trim).unwrap_or("");
        if !domain.is_empty() && valid_hostname(domain).is_err() {
            bail!("ngrok domain must be a bare hostname like my-app.ngrok-free.app");
        }
        self.core.vault.set_setting("tunnel.provider", provider)?;
        if ngrok_domain.is_some() {
            if domain.is_empty() {
                self.core.vault.del_setting("tunnel.ngrok_domain")?;
            } else {
                self.core.vault.set_setting("tunnel.ngrok_domain", &domain.to_lowercase())?;
            }
        }
        if !self.permanent() {
            self.core.vault.set_setting("tunnel.autostart", "0")?;
        }
        self.emit();
        Ok(())
    }

    // ───────── status ─────────

    pub fn status(&self) -> Value {
        let t = self.tunnel.lock().unwrap();
        let mcp_url = t.url.as_ref().map(|u| format!("{u}/mcp"));
        json!({
            "provider": self.provider(),
            "ngrokDomain": self.ngrok_domain(),
            "ngrokAuthtoken": self.setting("tunnel.ngrok_authtoken").is_some(),
            "cfMode": self.cf_mode(),
            "cfHostname": self.setting("tunnel.cf_hostname"),
            "cfZone": self.setting("tunnel.cf_zone"),
            "cfLoggedIn": Self::cf_cert().exists(),
            "permanent": self.permanent(),
            // The permanent hostname has been reached through the internet at least once.
            "verified": self.permanent_host().is_some_and(|h| self.setting("tunnel.verified_host").as_deref() == Some(h.as_str())),
            "autostart": self.autostart(),
            "installed": {
                "cloudflared": wakers::find_bin("cloudflared").is_some(),
                "ngrok": wakers::find_bin("ngrok").is_some(),
            },
            "paused": self.paused(),
            "state": t.phase.as_str(),
            "url": t.url,
            "mcpUrl": mcp_url,
            "error": t.error,
            "port": port(),
            "http": *self.http.lock().unwrap(),
            "tools": REMOTE_TOOLS,
        })
    }

    fn emit(&self) {
        self.core.emit("tunnel", self.status());
    }

    // ───────── tunnel ─────────

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let provider = self.provider();
        let (bin_name, hint) = if provider == "ngrok" { ("ngrok", "brew install ngrok") } else { ("cloudflared", "brew install cloudflared") };
        let Some(bin) = wakers::find_bin(bin_name) else {
            bail!("{bin_name} is not installed. Install it with: {hint}");
        };
        if self.http.lock().unwrap().is_none() {
            bail!("the local HTTP endpoint is not running (is port {} already in use?)", port());
        }
        // Every bot gets its token before anything is exposed.
        let bots: Vec<String> = self.core.vault.providers()?.into_iter().filter(|p| p["family"] == "grok").filter_map(|p| p["id"].as_str().map(String::from)).collect();
        for b in bots {
            self.token(&b)?;
        }

        let port = port();
        let local = format!("http://127.0.0.1:{port}");
        let metrics = format!("127.0.0.1:{}", metrics_port(port));
        let mut cmd = Command::new(bin);
        cmd.env("PATH", wakers::spawn_path());
        // For permanent tunnels the URL is known up front; we only wait for the connection.
        let mut known: Option<String> = None;
        if provider == "ngrok" {
            cmd.args(["http", &port.to_string(), "--log=stdout", "--log-format=json"]);
            if let Some(d) = self.ngrok_domain() {
                cmd.args(["--url", &d]);
                known = Some(format!("https://{d}"));
            }
            if self.setting("tunnel.ngrok_authtoken").is_some() {
                if let Some(t) = secrets::get(NGROK_TOKEN_SECRET)? {
                    cmd.env("NGROK_AUTHTOKEN", t); // env, not argv: invisible to `ps`
                }
            }
        } else {
            // A fixed metrics port lets a relaunched app find this tunnel again (see `adopt`).
            cmd.args(["tunnel", "--no-autoupdate", "--metrics", &metrics]);
            let host = self.setting("tunnel.cf_hostname");
            match (self.cf_mode().as_str(), host) {
                ("named", Some(h)) => {
                    let id = self.setting("tunnel.cf_tunnel").ok_or_else(|| anyhow!("no Cloudflare tunnel configured; set it up again"))?;
                    cmd.args(["run", "--url", &local, &id]);
                    known = Some(format!("https://{h}"));
                }
                ("token", Some(h)) => {
                    let t = secrets::get(CF_TOKEN_SECRET)?.ok_or_else(|| anyhow!("the Cloudflare tunnel token is missing from the keychain; paste it again"))?;
                    cmd.env("TUNNEL_TOKEN", t).args(["run", "--url", &local]);
                    known = Some(format!("https://{h}"));
                }
                _ => {
                    cmd.args(["--url", &local]);
                }
            }
        }
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);

        let kill = Arc::new(Notify::new());
        let (done_tx, done_rx) = oneshot::channel();
        let epoch = {
            let mut t = self.tunnel.lock().unwrap();
            if matches!(t.phase, Phase::Starting | Phase::Running) {
                bail!("the tunnel is already running");
            }
            let mut child = cmd.spawn().map_err(|e| anyhow!("could not start {bin_name}: {e}"))?;
            t.epoch += 1;
            t.phase = Phase::Starting;
            t.url = None;
            t.error = None;
            t.last_log = None;
            t.kill = Some(kill.clone());
            t.done = Some(done_rx);
            let epoch = t.epoch;

            if let Some(o) = child.stdout.take() {
                self.clone().scan(o, provider.clone(), known.clone(), epoch);
            }
            if let Some(e) = child.stderr.take() {
                self.clone().scan(e, provider.clone(), known.clone(), epoch);
            }
            let me = self.clone();
            tokio::spawn(async move {
                let died = tokio::select! {
                    r = child.wait() => Some(r),
                    _ = kill.notified() => { let _ = child.kill().await; None }
                };
                me.finished(epoch, died.is_some());
                let _ = done_tx.send(());
            });
            epoch
        };
        // Give up if no public URL shows up.
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(START_TIMEOUT).await;
            let kill = {
                let mut t = me.tunnel.lock().unwrap();
                if t.epoch == epoch && t.phase == Phase::Starting {
                    let log = t.last_log.clone().unwrap_or_default();
                    t.error = Some(format!("{bin_name} did not connect within {}s. {log}", START_TIMEOUT.as_secs()).trim().to_string());
                    t.kill.clone()
                } else {
                    None
                }
            };
            if let Some(k) = kill {
                k.notify_one();
            }
        });
        self.emit();
        Ok(())
    }

    /// Take over a tunnel left running by a previous app process (the app was restarted or
    /// crashed), so the URL already pasted into Grok keeps working. Returns true if adopted.
    pub async fn adopt(self: &Arc<Self>) -> bool {
        let port = port();
        let http = reqwest::Client::builder().timeout(Duration::from_secs(2)).build();
        let Ok(http) = http else { return false };
        let mut found: Option<(u32, &str, String)> = None;

        let permanent_cf = self.provider() == "cloudflared" && self.permanent();
        if permanent_cf {
            // The hostname is in settings; just find the process we launched.
            if let (Some(pid), Some(h)) = (
                find_pid(&format!("cloudflared tunnel --no-autoupdate --metrics 127.0.0.1:{} run", metrics_port(port))),
                self.setting("tunnel.cf_hostname"),
            ) {
                found = Some((pid, "cloudflared", format!("https://{h}")));
            }
        } else if let Some(pid) = find_pid(&format!("cloudflared tunnel --no-autoupdate( --metrics [^ ]+)? --url http://127.0.0.1:{port}")) {
            // Older launches used cloudflared's default metrics ports (20241-20245).
            for mp in std::iter::once(metrics_port(port)).chain(20241..=20245) {
                let Ok(r) = http.get(format!("http://127.0.0.1:{mp}/quicktunnel")).send().await else { continue };
                let host = r.json::<Value>().await.ok().and_then(|v| v["hostname"].as_str().map(String::from)).unwrap_or_default();
                if host.ends_with(".trycloudflare.com") {
                    found = Some((pid, "cloudflared", format!("https://{host}")));
                    break;
                }
            }
        }
        if found.is_none() {
            if let Some(pid) = find_pid(&format!("ngrok http {port}")) {
                if let Ok(r) = http.get("http://127.0.0.1:4040/api/tunnels").send().await {
                    let v: Value = r.json().await.unwrap_or(Value::Null);
                    let url = v["tunnels"].as_array().into_iter().flatten().find_map(|t| {
                        let u = t["public_url"].as_str()?;
                        let addr = t["config"]["addr"].as_str().unwrap_or("");
                        (u.starts_with("https://") && addr.ends_with(&format!(":{port}"))).then(|| u.to_string())
                    });
                    if let Some(u) = url {
                        found = Some((pid, "ngrok", u));
                    }
                }
            }
        }
        let Some((pid, provider, url)) = found else { return false };

        let kill = Arc::new(Notify::new());
        let (done_tx, done_rx) = oneshot::channel();
        let epoch = {
            let mut t = self.tunnel.lock().unwrap();
            if matches!(t.phase, Phase::Starting | Phase::Running) {
                return false;
            }
            t.epoch += 1;
            t.phase = Phase::Running;
            t.url = Some(url);
            t.error = None;
            t.last_log = None;
            t.kill = Some(kill.clone());
            t.done = Some(done_rx);
            t.epoch
        };
        let _ = self.core.vault.set_setting("tunnel.provider", provider);
        tracing::info!("adopted running {provider} tunnel (pid {pid})");
        // Not our child any more, so supervise it by pid.
        let me = self.clone();
        tokio::spawn(async move {
            let died = loop {
                tokio::select! {
                    _ = kill.notified() => {
                        let _ = tokio::process::Command::new("kill").arg(pid.to_string()).status().await;
                        for _ in 0..30 {
                            if !alive(pid) { break; }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        break false;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(3)) => {
                        if !alive(pid) { break true; }
                    }
                }
            };
            me.finished(epoch, died);
            let _ = done_tx.send(());
        });
        self.emit();
        true
    }

    pub async fn stop(&self) {
        let (kill, done) = {
            let mut t = self.tunnel.lock().unwrap();
            t.epoch += 1; // the supervisor's late report is ignored
            t.phase = Phase::Stopped;
            t.url = None;
            t.error = None;
            (t.kill.take(), t.done.take())
        };
        if let Some(k) = kill {
            k.notify_one();
        }
        if let Some(d) = done {
            let _ = tokio::time::timeout(Duration::from_secs(3), d).await;
        }
        self.emit();
    }

    /// Supervisor callback: the tunnel process is gone.
    fn finished(&self, epoch: u64, died: bool) {
        {
            let mut t = self.tunnel.lock().unwrap();
            if t.epoch != epoch {
                return;
            }
            let had_url = t.url.is_some();
            t.url = None;
            t.kill = None;
            t.done = None;
            if died || t.error.is_some() {
                let why = t.error.clone().or_else(|| t.last_log.clone()).unwrap_or_else(|| "the tunnel process exited".into());
                t.phase = if had_url { Phase::Stopped } else { Phase::Failed };
                t.error = Some(why);
            } else {
                t.phase = Phase::Stopped;
            }
        }
        self.emit();
    }

    fn scan<R: AsyncRead + Unpin + Send + 'static>(self: Arc<Self>, r: R, provider: String, known: Option<String>, epoch: u64) {
        tokio::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut changed = false;
                {
                    let mut t = self.tunnel.lock().unwrap();
                    if t.epoch != epoch {
                        return;
                    }
                    if let Some(msg) = notable_error(&provider, &line) {
                        t.last_log = Some(msg);
                    }
                    if t.url.is_none() {
                        let hit = match &known {
                            // Named/token Cloudflare tunnels: the hostname is ours; wait for a live edge connection.
                            Some(u) if provider == "cloudflared" => line.contains("Registered tunnel connection").then(|| u.clone()),
                            _ => find_public_url(&provider, &line),
                        };
                        if let Some(url) = hit {
                            t.url = Some(url);
                            t.phase = Phase::Running;
                            changed = true;
                        }
                    }
                }
                if changed {
                    self.emit();
                }
            }
        });
    }

    // ───────── HTTP ─────────

    /// Keep trying to bind the HTTP port until it's free (another AgentMux process may hold it).
    pub async fn serve_http_retrying(self: Arc<Self>) {
        let mut warned = false;
        loop {
            match self.clone().serve_http().await {
                Ok(()) => break,
                Err(e) => {
                    if !warned {
                        tracing::info!("HTTP gateway waiting: {e}");
                        warned = true;
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    /// Stop the AgentMux process holding our HTTP port (an old daemon, a dev build) so this one
    /// can bind it. Never touches a process that isn't AgentMux.
    pub async fn take_over_port(&self) -> Result<String> {
        if self.http_bound() {
            return Ok("AgentMux already has the port.".into());
        }
        let Some(h) = port_holder() else {
            return Ok("The port is free; AgentMux will pick it up in a moment.".into());
        };
        if !h.agentmux {
            bail!("Port {} is used by {} (pid {}), which isn't AgentMux, so it was left alone. Quit that program, or start AgentMux with a different AGENTMUX_PORT.", port(), h.path, h.pid);
        }
        if h.pid == std::process::id() {
            bail!("this process already holds the port");
        }
        let kill = |sig: &str| std::process::Command::new("kill").args([sig, &h.pid.to_string()]).status();
        kill("-TERM")?;
        for i in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if port_holder().is_none_or(|x| x.pid != h.pid) {
                break;
            }
            if i == 29 {
                let _ = kill("-KILL");
            }
        }
        if port_holder().is_some_and(|x| x.pid == h.pid) {
            bail!("pid {} did not exit", h.pid);
        }
        tracing::info!("stopped AgentMux process {} ({}) holding port {}", h.pid, h.path, port());
        Ok(format!("Stopped the other AgentMux process (pid {}, {}). This app now serves port {}.", h.pid, h.path, port()))
    }

    pub async fn serve_http(self: Arc<Self>) -> Result<()> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port()));
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| anyhow!("cannot listen on {addr}: {e}"))?;
        *self.http.lock().unwrap() = Some(addr.to_string());
        tracing::info!("agentmux http gateway on http://{addr}/mcp");
        self.emit();
        let app = Router::new().route("/mcp", post(mcp_post)).route("/healthz", get(healthz)).with_state(self);
        axum::serve(listener, app).await?;
        Ok(())
    }

    fn host_allowed(&self, headers: &HeaderMap) -> bool {
        let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else { return false };
        let host = host.to_lowercase();
        let name = host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h).unwrap_or(&host);
        if matches!(name, "localhost" | "127.0.0.1" | "[::1]") {
            return true;
        }
        let t = self.tunnel.lock().unwrap();
        t.url.as_deref().and_then(|u| u.strip_prefix("https://")).is_some_and(|h| h.eq_ignore_ascii_case(name))
    }

    /// Crude brute-force / flood guard: 300 requests and 10 bad tokens per minute.
    fn admit(&self) -> bool {
        let mut l = self.limiter.lock().unwrap();
        if l.window.elapsed() > Duration::from_secs(60) {
            *l = Limiter { window: Instant::now(), total: 0, bad: 0 };
        }
        l.total += 1;
        l.total <= 300 && l.bad < 10
    }

    /// Sliding one-hour budget of new questions per bot.
    fn ask_allowed(&self, agent: &str) -> bool {
        let mut m = self.asks.lock().unwrap();
        let v = m.entry(agent.to_string()).or_default();
        v.retain(|t| t.elapsed() < Duration::from_secs(3600));
        if v.len() >= MAX_ASKS_PER_HOUR {
            return false;
        }
        v.push(Instant::now());
        true
    }

    fn note_bad_token(&self) {
        self.limiter.lock().unwrap().bad += 1;
    }
}

async fn healthz() -> Json<Value> {
    Json(json!({"ok": true, "service": "agentmux"}))
}

async fn mcp_post(State(g): State<Arc<Gateway>>, headers: HeaderMap, body: Bytes) -> Response {
    if g.paused() {
        return (StatusCode::SERVICE_UNAVAILABLE, "AgentMux is turned off").into_response();
    }
    if !g.host_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "unexpected Host header").into_response();
    }
    if !g.admit() {
        return (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
    }
    let presented = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    let Some(agent) = g.identify(presented) else {
        g.note_bad_token();
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")], "invalid or missing bearer token").into_response();
    };
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}))).into_response(),
    };
    // The tool set is decided per request, so flipping the switch takes effect immediately.
    let mut tools: Vec<&str> = REMOTE_TOOLS.to_vec();
    if g.core.vault.can_ask(&agent) {
        tools.extend_from_slice(ASK_TOOLS);
    }
    let ctx = CallerCtx { agent: agent.clone(), cwd: config::home().to_string_lossy().to_string(), hop: 0 };
    let handle = |mut item: Value| {
        let g = g.clone();
        let (ctx, tools, agent) = (ctx.clone(), tools.clone(), agent.clone());
        async move {
            if item["method"] == "tools/call" && item["params"]["name"] == "mux_ask_peer" && tools.contains(&"mux_ask_peer") {
                // A remote bot can't pick a room or session: its questions go to the neutral home
                // room, so no workspace diff or notes ride along, and it can't write into someone's thread.
                if let Some(a) = item["params"]["arguments"].as_object_mut() {
                    a.remove("room");
                    a.remove("session");
                }
                if !g.ask_allowed(&agent) {
                    let id = item.get("id").cloned().unwrap_or(Value::Null);
                    return Some(json!({"jsonrpc": "2.0", "id": id, "result": {"isError": true,
                        "content": [{"type": "text", "text": format!("question limit reached ({MAX_ASKS_PER_HOUR} per hour); try again later")}]}}));
                }
            }
            mcp::handle_scoped(&g.core, &ctx, item, Some(&tools)).await
        }
    };
    match req {
        Value::Array(items) => {
            let mut out = vec![];
            for item in items {
                if let Some(r) = handle(item).await {
                    out.push(r);
                }
            }
            if out.is_empty() { StatusCode::ACCEPTED.into_response() } else { Json(Value::Array(out)).into_response() }
        }
        item => match handle(item).await {
            Some(r) => Json(r).into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

fn new_token() -> String {
    format!("agmx_{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// The hostname `cloudflared tunnel route dns` reports, e.g. "Added CNAME mux.example.com which
/// will route to this tunnel" or "mux.example.com is already configured to route to your tunnel".
pub fn routed_hostname(log: &str) -> Option<String> {
    for line in log.lines() {
        if let Some(rest) = line.split("Added CNAME ").nth(1) {
            return rest.split_whitespace().next().map(|h| h.trim_end_matches('.').to_lowercase());
        }
        if let Some(i) = line.find(" is already configured to route to your tunnel") {
            return line[..i].split_whitespace().last().map(|h| h.trim_end_matches('.').to_lowercase());
        }
    }
    None
}

/// A bare DNS hostname with at least one dot (no scheme, path or port).
fn valid_hostname(raw: &str) -> Result<String> {
    let h = raw.trim().trim_end_matches('.').to_lowercase();
    let ok = h.len() <= 253
        && h.contains('.')
        && h.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    if !ok {
        bail!("enter a hostname like mux.example.com (no https:// or path)");
    }
    Ok(h)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The public https URL in one line of tunnel output, if there is one.
pub fn find_public_url(provider: &str, line: &str) -> Option<String> {
    if provider == "ngrok" {
        let v: Value = serde_json::from_str(line).ok()?;
        if v["msg"] == "started tunnel" {
            let u = v["url"].as_str()?;
            return u.starts_with("https://").then(|| u.trim_end_matches('/').to_string());
        }
        return None;
    }
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == ':' || c == '/' || c == '.' || c == '-')).unwrap_or(rest.len());
    let url = &rest[..end];
    (url.ends_with(".trycloudflare.com") && url != "https://api.trycloudflare.com").then(|| url.to_string())
}

/// A line worth showing the user if the tunnel fails to come up.
fn notable_error(provider: &str, line: &str) -> Option<String> {
    if provider == "ngrok" {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            let e = v["err"].as_str().filter(|e| !e.is_empty() && *e != "<nil>")?;
            return Some(e.lines().next().unwrap_or(e).chars().take(300).collect());
        }
        return line.contains("ERR_NGROK").then(|| line.chars().take(300).collect());
    }
    (line.contains(" ERR ") || line.contains("failed")).then(|| line.chars().take(300).collect())
}

/// Who is listening on the gateway port, if anyone.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PortHolder {
    pub pid: u32,
    pub path: String,
    pub agentmux: bool,
}

pub fn port_holder() -> Option<PortHolder> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{}", port()), "-sTCP:LISTEN", "-Fp"])
        .output()
        .ok()?;
    let pid: u32 = String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix('p')?.parse().ok())?;
    let ps = std::process::Command::new("ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok()?;
    let path = String::from_utf8_lossy(&ps.stdout).trim().to_string();
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    Some(PortHolder { pid, agentmux: matches!(name.as_str(), "agentmux" | "agentmux-app" | "agentmux-cli"), path })
}
