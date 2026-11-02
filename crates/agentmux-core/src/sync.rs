//! Free backup and multi-Mac sync through a git repo you own (GitHub, GitLab, a bare repo on a
//! drive...). AgentMux never talks to a server of ours.
//!
//! The vault is exported to plain JSON and Markdown in `~/.agentmux/sync`, a git clone of your
//! repo, then committed and pushed with your own git credentials. Secrets never go in: bot tokens,
//! webhook keys and tunnel tokens stay in the macOS Keychain, settings are an allow-list, git diffs
//! and terminal output are off unless you opt in, and obvious credentials in message text are
//! redacted. Importing only adds rows by id, so syncing can never delete or overwrite local data.

use crate::{config, vault::Vault, wakers};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const REMOTE: &str = "sync.remote";
const INCLUDE_DIFFS: &str = "sync.include_diffs";
const AUTO: &str = "sync.auto";
const LAST: &str = "sync.last";
const LAST_ERROR: &str = "sync.last_error";
const VISIBILITY: &str = "sync.visibility";
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

pub fn dir() -> PathBuf {
    config::home().join("sync")
}

// ───────── redaction ─────────

/// Best-effort removal of obvious credentials from text that is about to leave the machine.
pub fn redact(s: &str) -> String {
    let mut text = s.to_string();
    // PEM blocks (private keys, certificates).
    while let Some(a) = text.find("-----BEGIN ") {
        let end = match text[a..].find("-----END ") {
            Some(e) => {
                let from = a + e + 9;
                text[from..].find("-----").map(|x| from + x + 5).unwrap_or(text.len())
            }
            None => text.len(),
        };
        text.replace_range(a..end, "[redacted key]");
    }
    let is_delim = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '|');
    let mut out = String::with_capacity(text.len());
    let mut prev_bearer = false;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let (word, tail) = match rest.find(is_delim) {
            Some(i) => {
                let d = rest[i..].chars().next().unwrap();
                (&rest[..i], Some((d, &rest[i + d.len_utf8()..])))
            }
            None => (rest, None),
        };
        let secret = looks_secret(word) || (prev_bearer && word.len() >= 16 && word.chars().all(|c| c.is_ascii_alphanumeric() || "-._~+/=".contains(c)));
        out.push_str(if secret { "[redacted]" } else { word });
        prev_bearer = word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("authorization:");
        match tail {
            Some((d, t)) => {
                out.push(d);
                if !d.is_whitespace() {
                    prev_bearer = false;
                }
                rest = t;
            }
            None => break,
        }
    }
    out
}

fn looks_secret(w: &str) -> bool {
    let w = w.trim_matches(|c: char| matches!(c, '.' | ':' | '=' | '!' | '?'));
    let alnum = |t: &str| t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    const PREFIXES: &[(&str, usize)] = &[
        ("gho_", 20),
        ("ghp_", 20),
        ("ghs_", 20),
        ("ghu_", 20),
        ("github_pat_", 20),
        ("glpat-", 16),
        ("sk-", 20),
        ("xoxb-", 16),
        ("xoxp-", 16),
        ("agmx_", 20),
        ("AKIA", 16),
        ("AIza", 30),
    ];
    for (p, min) in PREFIXES {
        if let Some(t) = w.strip_prefix(p) {
            if w.len() >= *min && alnum(t) {
                return true;
            }
        }
    }
    // JWTs: three base64url parts.
    if w.starts_with("eyJ") && w.len() > 40 && w.matches('.').count() == 2 {
        return true;
    }
    // KEY=value style assignments of well-known secret names.
    if let Some((k, v)) = w.split_once('=') {
        let k = k.to_ascii_uppercase();
        if v.len() >= 12 && ["SECRET", "TOKEN", "PASSWORD", "API_KEY", "APIKEY", "PRIVATE_KEY"].iter().any(|n| k.contains(n)) {
            return true;
        }
    }
    false
}

fn redact_value(v: &mut Value) {
    match v {
        Value::String(s) => *s = redact(s),
        Value::Array(a) => a.iter_mut().for_each(redact_value),
        Value::Object(o) => o.values_mut().for_each(redact_value),
        _ => {}
    }
}

// ───────── remotes ─────────

/// Accepts https, ssh, scp-style and local (bare repo on a disk or synced folder) remotes.
/// Refuses anything that could smuggle credentials or a git transport helper.
pub fn validate_remote(raw: &str) -> Result<String> {
    let r = raw.trim();
    if r.is_empty() {
        bail!("enter the git remote, for example git@github.com:you/agentmux-backup.git");
    }
    if r.chars().any(|c| c.is_control() || c.is_whitespace()) || r.starts_with('-') || r.contains("::") {
        bail!("that doesn't look like a git remote URL");
    }
    if let Some(rest) = r.strip_prefix("https://") {
        let authority = rest.split('/').next().unwrap_or("");
        if authority.contains('@') {
            bail!("don't put a username or token in the URL; use your normal git login (SSH key or credential helper)");
        }
        if !rest.contains('/') {
            bail!("the URL needs a repository path, like https://github.com/you/agentmux-backup.git");
        }
        return Ok(r.to_string());
    }
    if r.starts_with("http://") {
        bail!("use https:// or SSH for the remote");
    }
    if r.starts_with("ssh://") || r.starts_with("file://") || r.starts_with('/') {
        return Ok(r.to_string());
    }
    // scp-style: user@host:path
    if let Some((left, path)) = r.split_once(':') {
        if left.contains('@') && !left.contains('/') && !path.is_empty() {
            return Ok(r.to_string());
        }
    }
    bail!("use a remote like git@github.com:you/agentmux-backup.git or https://github.com/you/agentmux-backup.git")
}

/// `(host, path)` for GitHub-style remotes, so the repository's visibility can be checked.
pub fn host_and_path(remote: &str) -> Option<(String, String)> {
    let strip = |p: &str| p.trim_end_matches('/').trim_end_matches(".git").trim_start_matches('/').to_string();
    if let Some(rest) = remote.strip_prefix("https://").or_else(|| remote.strip_prefix("ssh://")) {
        let (auth, path) = rest.split_once('/')?;
        let host = auth.rsplit('@').next()?.split(':').next()?;
        return Some((host.to_lowercase(), strip(path)));
    }
    let (left, path) = remote.split_once(':')?;
    if !left.contains('/') && left.contains('@') {
        return Some((left.rsplit('@').next()?.to_lowercase(), strip(path)));
    }
    None
}

/// Your conversations must not go to a public repo. For GitHub and GitLab this asks their public
/// API (without credentials): a repo it can see is public. Returns "private" or "unverified".
pub async fn check_private(remote: &str) -> Result<&'static str> {
    let Some((host, path)) = host_and_path(remote) else { return Ok("unverified") };
    let url = match host.as_str() {
        "github.com" => format!("https://api.github.com/repos/{path}"),
        "gitlab.com" => format!("https://gitlab.com/api/v4/projects/{}", path.replace('/', "%2F")),
        _ => return Ok("unverified"),
    };
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).user_agent("agentmux-sync").build()?;
    match client.get(url).send().await {
        Ok(r) if r.status().is_success() => bail!("{host}/{path} is a PUBLIC repository. Sync would publish your conversations. Make it private first, or create a new private repo."),
        Ok(r) if r.status().as_u16() == 404 => Ok("private"),
        _ => Ok("unverified"),
    }
}

// ───────── git ─────────

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let bin = wakers::find_bin("git").unwrap_or_else(|| PathBuf::from("git"));
    let mut cmd = Command::new(bin);
    cmd.arg("-C").arg(dir);
    // Fixed identity, no hooks or helpers from the global config that could prompt or run code.
    cmd.args(["-c", "user.name=AgentMux", "-c", "user.email=agentmux@localhost", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"]);
    cmd.args(args);
    cmd.env("PATH", wakers::spawn_path()).env("GIT_TERMINAL_PROMPT", "0").env("GIT_ASKPASS", "echo");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=15");
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| anyhow!("git {} timed out", args.first().copied().unwrap_or("")))?
        .map_err(|e| anyhow!("could not run git: {e}. Install Xcode command line tools (xcode-select --install)"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
        bail!("git {} failed: {}", args.first().copied().unwrap_or(""), friendly(&tail));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn friendly(msg: &str) -> String {
    let l = msg.to_lowercase();
    if l.contains("permission denied") || l.contains("publickey") || l.contains("authentication failed") || l.contains("could not read username") {
        format!("{msg}. Check that this Mac can push to the repo with your normal git login (try `git ls-remote <repo>` in a terminal).")
    } else if l.contains("not found") || l.contains("does not appear to be a git repository") || l.contains("repository not found") {
        format!("{msg}. Create the (empty, private) repository first and check the URL.")
    } else {
        msg.to_string()
    }
}

async fn ensure_repo(dir: &Path, remote: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    if !dir.join(".git").exists() {
        git(dir, &["init", "-q", "-b", "main"]).await?;
    }
    if git(dir, &["remote", "get-url", "origin"]).await.is_ok() {
        git(dir, &["remote", "set-url", "origin", remote]).await?;
    } else {
        git(dir, &["remote", "add", "origin", remote]).await?;
    }
    Ok(())
}

// ───────── repo files <-> vault ─────────

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect()
}

fn write_json(path: &Path, v: &Value) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(v)? + "\n")?;
    Ok(())
}

/// Rewrite the repo's contents from the vault. Everything is regenerated, so the files always
/// equal "what this Mac knows" (which, after a pull, includes every other Mac's data too).
fn write_repo(dir: &Path, vault: &Vault, include_diffs: bool) -> Result<()> {
    for sub in ["rooms", "sessions", "threads"] {
        let p = dir.join(sub);
        if p.exists() {
            std::fs::remove_dir_all(&p)?;
        }
    }
    let mut d = vault.dump_for_sync(include_diffs)?;
    redact_value(&mut d);
    let list = |k: &str| d[k].as_array().cloned().unwrap_or_default();
    let by = |rows: &[Value], key: &str, id: &str| rows.iter().filter(|r| r[key] == id).cloned().collect::<Vec<_>>();

    write_json(
        &dir.join("config.json"),
        &json!({"version": 1, "exportedAt": chrono::Utc::now().to_rfc3339(), "providers": d["providers"], "settings": d["settings"]}),
    )?;
    let (members, context, sessions, messages, tickets) = (list("room_members"), list("context"), list("sessions"), list("messages"), list("tickets"));
    for room in list("rooms") {
        let rid = room["id"].as_str().unwrap_or("room").to_string();
        write_json(
            &dir.join("rooms").join(format!("{}.json", safe(&rid))),
            &json!({"room": room, "members": by(&members, "room_id", &rid), "context": by(&context, "room_id", &rid)}),
        )?;
        for s in by(&sessions, "room_id", &rid) {
            let sid = s["id"].as_str().unwrap_or("session").to_string();
            write_json(
                &dir.join("sessions").join(safe(&rid)).join(format!("{}.json", safe(&sid))),
                &json!({"session": s, "messages": by(&messages, "session_id", &sid), "tickets": by(&tickets, "session_id", &sid)}),
            )?;
        }
        // A readable transcript built from the messages only. (The app's own .md export also holds the
        // shared git diff, which must not leave the machine unless include-diffs is on.)
        let mut md = format!("# {}\n", room["title"].as_str().unwrap_or(&rid));
        for ss in by(&sessions, "room_id", &rid) {
            md.push_str(&format!("\n## {}\n", ss["title"].as_str().unwrap_or("Session")));
            for m in by(&messages, "session_id", ss["id"].as_str().unwrap_or("")) {
                md.push_str(&format!(
                    "\n**{} → {}** · {}\n\n{}\n",
                    m["sender"].as_str().unwrap_or("?"),
                    m["recipient"].as_str().unwrap_or("all"),
                    m["created_at"].as_str().unwrap_or(""),
                    m["text"].as_str().unwrap_or("")
                ));
            }
        }
        std::fs::create_dir_all(dir.join("threads"))?;
        std::fs::write(dir.join("threads").join(format!("{}.md", safe(&rid))), md)?;
    }
    std::fs::write(
        dir.join("README.md"),
        "# AgentMux sync\n\nBackup of an [AgentMux](https://github.com/agentmuxhq/agentmux-mcp) vault: rooms, sessions, messages and setup.\n\n\
Keep this repository **private**. It never contains tokens, webhook keys or tunnel credentials (those stay in each Mac's Keychain), and obvious credentials in message text are redacted. \
To restore on another Mac: install AgentMux, open Settings → Sync, enter this repository and press Sync now.\n",
    )?;
    Ok(())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Gather the repo's files back into one dump for `Vault::restore_from_sync`.
fn read_repo(dir: &Path) -> Value {
    let mut d = json!({"providers": [], "settings": [], "rooms": [], "room_members": [], "context": [], "sessions": [], "messages": [], "tickets": []});
    let push = |d: &mut Value, k: &str, items: &Value| {
        if let (Some(a), Some(b)) = (d[k].as_array_mut(), items.as_array()) {
            a.extend(b.iter().cloned());
        }
    };
    if let Some(c) = read_json(&dir.join("config.json")) {
        push(&mut d, "providers", &c["providers"]);
        push(&mut d, "settings", &c["settings"]);
    }
    let files = |p: PathBuf| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect();
        v.sort();
        v
    };
    for f in files(dir.join("rooms")).into_iter().filter(|f| f.extension().is_some_and(|e| e == "json")) {
        if let Some(r) = read_json(&f) {
            if let Some(a) = d["rooms"].as_array_mut() {
                a.push(r["room"].clone());
            }
            push(&mut d, "room_members", &r["members"]);
            push(&mut d, "context", &r["context"]);
        }
    }
    for room_dir in files(dir.join("sessions")).into_iter().filter(|p| p.is_dir()) {
        for f in files(room_dir).into_iter().filter(|f| f.extension().is_some_and(|e| e == "json")) {
            if let Some(s) = read_json(&f) {
                if let Some(a) = d["sessions"].as_array_mut() {
                    a.push(s["session"].clone());
                }
                push(&mut d, "messages", &s["messages"]);
                push(&mut d, "tickets", &s["tickets"]);
            }
        }
    }
    d
}

// ───────── public API ─────────

fn flag(v: &Vault, key: &str) -> bool {
    v.setting(key).ok().flatten().as_deref() == Some("1")
}

pub fn status(v: &Vault) -> Value {
    let remote = v.setting(REMOTE).ok().flatten();
    json!({
        "configured": remote.is_some(),
        "remote": remote,
        "includeDiffs": flag(v, INCLUDE_DIFFS),
        "auto": flag(v, AUTO),
        "last": v.setting(LAST).ok().flatten(),
        "error": v.setting(LAST_ERROR).ok().flatten().filter(|e| !e.is_empty()),
        "visibility": v.setting(VISIBILITY).ok().flatten(),
        "gitInstalled": wakers::find_bin("git").is_some(),
        "dir": dir().to_string_lossy(),
    })
}

pub fn set_options(v: &Vault, include_diffs: bool, auto: bool) -> Result<()> {
    v.set_setting(INCLUDE_DIFFS, if include_diffs { "1" } else { "0" })?;
    v.set_setting(AUTO, if auto { "1" } else { "0" })
}

/// Point sync at a repo. Refuses public GitHub/GitLab repos. Doesn't sync yet.
pub async fn connect(v: &Vault, remote: &str, include_diffs: bool, auto: bool) -> Result<Value> {
    let remote = validate_remote(remote)?;
    if wakers::find_bin("git").is_none() {
        bail!("git isn't installed. Run `xcode-select --install` in a terminal, then try again");
    }
    let visibility = check_private(&remote).await?;
    ensure_repo(&dir(), &remote).await?;
    v.set_setting(REMOTE, &remote)?;
    v.set_setting(VISIBILITY, visibility)?;
    v.del_setting(LAST_ERROR)?;
    set_options(v, include_diffs, auto)?;
    Ok(status(v))
}

/// Forget the repo. The local clone is only a derived copy, so it is removed; the remote is untouched.
pub fn disconnect(v: &Vault) -> Result<Value> {
    for k in [REMOTE, LAST, LAST_ERROR, VISIBILITY, AUTO, INCLUDE_DIFFS] {
        v.del_setting(k)?;
    }
    let d = dir();
    if d.join(".git").exists() {
        let _ = std::fs::remove_dir_all(&d);
    }
    Ok(status(v))
}

/// Pull what other Macs wrote, merge it in, write the union, commit and push.
pub async fn sync_now(v: &Vault) -> Result<Value> {
    let result = run(v).await;
    match &result {
        Ok(_) => {
            v.set_setting(LAST, &chrono::Utc::now().to_rfc3339())?;
            v.del_setting(LAST_ERROR)?;
        }
        Err(e) => v.set_setting(LAST_ERROR, &e.to_string())?,
    }
    result
}

async fn run(v: &Vault) -> Result<Value> {
    let remote = v.setting(REMOTE)?.ok_or_else(|| anyhow!("sync isn't set up yet"))?;
    let include = flag(v, INCLUDE_DIFFS);
    let dir = dir();
    ensure_repo(&dir, &remote).await?;
    // Re-check on every run: someone may have made the repo public since.
    check_private(&remote).await?;

    let mut imported = json!({"rooms": 0, "sessions": 0, "messages": 0, "bots": 0});
    let mut pushed = false;
    for attempt in 0..2 {
        git(&dir, &["fetch", "-q", "origin"]).await?;
        if git(&dir, &["rev-parse", "--verify", "-q", "origin/main"]).await.is_ok() {
            // The clone is a derived copy: anything local is already in the vault and gets rewritten below.
            git(&dir, &["reset", "-q", "--hard", "origin/main"]).await?;
            let stats = v.restore_from_sync(&read_repo(&dir))?;
            for k in ["rooms", "sessions", "messages", "bots"] {
                imported[k] = json!(imported[k].as_i64().unwrap_or(0) + stats[k].as_i64().unwrap_or(0));
            }
        }
        write_repo(&dir, v, include)?;
        git(&dir, &["add", "-A"]).await?;
        let changed = !git(&dir, &["status", "--porcelain"]).await?.trim().is_empty();
        if changed {
            git(&dir, &["commit", "-q", "-m", &format!("AgentMux sync {}", chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"))]).await?;
        }
        let ahead = git(&dir, &["rev-parse", "--verify", "-q", "HEAD"]).await.is_ok()
            && (git(&dir, &["rev-parse", "--verify", "-q", "origin/main"]).await.is_err()
                || git(&dir, &["rev-list", "--count", "origin/main..HEAD"]).await.map(|s| s.trim() != "0").unwrap_or(true));
        if !ahead {
            break;
        }
        match git(&dir, &["push", "-q", "origin", "HEAD:main"]).await {
            Ok(_) => {
                pushed = true;
                break;
            }
            // Another Mac pushed in between: fetch, merge and try once more.
            Err(e) if attempt == 0 && e.to_string().to_lowercase().contains("rejected") => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(json!({"imported": imported, "pushed": pushed}))
}
