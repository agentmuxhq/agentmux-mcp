//! Custom CLI agents without Rust: a TOML file in `~/.agentmux/providers/`.
//!
//! ```toml
//! id = "aider"
//! name = "Aider"
//! color = "#14B8A6"
//! detect = { binary = "aider" }
//! mcp = { kind = "json-merge", path = "~/.aider/mcp.json", key = "mcpServers.agentmux" }
//! wake = { binary = "aider", args = ["--message", "{prompt}", "--yes-always"], read_only_args = ["--dry-run"] }
//! ```
//! `mcp.kind = "command"` runs `add = [...]` / `remove = [...]` argv lists on the detected binary
//! instead (placeholder `{link}` is the AgentMux binary). Arguments never go through a shell.

use super::*;
use crate::wakers::{find_bin, spawn_path};
use serde::Deserialize;
use std::process::Command;

#[derive(Deserialize, Clone, Debug)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_color")]
    pub color: String,
    pub detect: Detect,
    pub mcp: Mcp,
    pub wake: Option<Wake>,
}

fn default_color() -> String {
    "#6B7280".into()
}

#[derive(Deserialize, Clone, Debug)]
pub struct Detect {
    pub binary: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Mcp {
    pub kind: String,
    pub path: Option<String>,
    pub key: Option<String>,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Wake {
    pub binary: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub read_only_args: Vec<String>,
}

const BUILTIN_IDS: [&str; 5] = ["cursor", "grok", "claude", "chatgpt", "gemini"];
const WAKE_PLACEHOLDERS: [&str; 3] = ["{prompt}", "{workspace}", "{ticket}"];

pub fn dir() -> PathBuf {
    crate::config::home().join("providers")
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest> {
        let m: Manifest = toml::from_str(text).map_err(|e| anyhow::anyhow!("invalid provider manifest: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        if crate::config::slug(&self.id) != self.id || self.id.is_empty() {
            bail!("id '{}' must be lowercase letters, digits and dashes", self.id);
        }
        if BUILTIN_IDS.contains(&self.id.as_str()) || self.id == "user" {
            bail!("id '{}' is taken by a built-in provider", self.id);
        }
        if self.name.trim().is_empty() {
            bail!("name is required");
        }
        match self.mcp.kind.as_str() {
            "json-merge" if self.mcp.path.is_some() && self.mcp.key.is_some() => {}
            "json-merge" => bail!("mcp.kind json-merge needs path and key"),
            "command" if !self.mcp.add.is_empty() && !self.mcp.remove.is_empty() => {}
            "command" => bail!("mcp.kind command needs add and remove"),
            k => bail!("unknown mcp.kind '{k}' (use json-merge or command)"),
        }
        if let Some(w) = &self.wake {
            for a in w.args.iter().chain(&w.read_only_args) {
                let mut rest = a.as_str();
                while let Some(i) = rest.find('{') {
                    let tail = &rest[i..];
                    let Some(end) = tail.find('}') else { bail!("unbalanced '{{' in argument '{a}'") };
                    if !WAKE_PLACEHOLDERS.contains(&&tail[..=end]) {
                        bail!("unknown placeholder {} (allowed: {})", &tail[..=end], WAKE_PLACEHOLDERS.join(" "));
                    }
                    rest = &tail[end + 1..];
                }
            }
            if !w.args.iter().any(|a| a.contains("{prompt}")) {
                bail!("wake.args must contain {{prompt}}");
            }
        }
        Ok(())
    }
}

pub fn load_all() -> Vec<Arc<dyn Provider>> {
    let Ok(rd) = std::fs::read_dir(dir()) else { return vec![] };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let m = std::fs::read_to_string(&p).ok().and_then(|t| Manifest::parse(&t).ok())?;
            Some(Arc::new(ManifestProvider(m)) as Arc<dyn Provider>)
        })
        .collect()
}

/// Validate and copy a manifest into `~/.agentmux/providers/<id>.toml`.
pub fn add(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)?;
    let m = Manifest::parse(&text)?;
    std::fs::create_dir_all(dir())?;
    crate::vault::atomic_write(&dir().join(format!("{}.toml", m.id)), &text)?;
    Ok(m.id)
}

pub fn remove(id: &str) -> Result<()> {
    let p = dir().join(format!("{id}.toml"));
    if !p.is_file() {
        bail!("no custom provider '{id}'");
    }
    Ok(std::fs::remove_file(p)?)
}

pub struct ManifestProvider(pub Manifest);

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

impl ManifestProvider {
    fn run(&self, args: &[String], link: &Path) -> Result<String> {
        let bin = find_bin(&self.0.detect.binary).ok_or_else(|| anyhow::anyhow!("{} not found", self.0.detect.binary))?;
        let args: Vec<String> = args.iter().map(|a| a.replace("{link}", &link.to_string_lossy())).collect();
        let out = Command::new(&bin).args(&args).env("PATH", spawn_path()).output()?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() {
            bail!("{} failed: {}", bin.display(), text.trim());
        }
        Ok(text.trim().to_string())
    }
}

impl Provider for ManifestProvider {
    fn info(&self) -> ProviderInfo {
        let m = &self.0;
        ProviderInfo { id: m.id.clone(), name: m.name.clone(), color: m.color.clone(), hint: "Custom provider · ~/.agentmux/providers".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: if self.0.wake.is_some() { "cli" } else { "inbox" }, context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        match find_bin(&self.0.detect.binary) {
            Some(b) => (true, b.display().to_string()),
            None => (false, format!("{} not found", self.0.detect.binary)),
        }
    }
    fn configured(&self) -> bool {
        match (self.0.mcp.kind.as_str(), &self.0.mcp.path, &self.0.mcp.key) {
            ("json-merge", Some(p), Some(k)) => std::fs::read_to_string(expand(p))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .is_some_and(|v| k.split('.').fold(&v, |c, part| &c[part]).is_object()),
            _ => false,
        }
    }
    fn install(&self, link: &Path) -> Result<String> {
        let m = &self.0.mcp;
        if m.kind == "command" {
            return self.run(&m.add, link).map(|_| format!("Registered 'agentmux' with {}.", self.0.name));
        }
        let entry = json!({"command": link.to_string_lossy(), "args": ["mcp", "--agent", self.0.id]});
        json_merge_server(&expand(m.path.as_deref().unwrap_or("")), m.key.as_deref().unwrap_or(""), entry, "Restart the app to load it.")
    }
    fn uninstall(&self) -> Result<String> {
        let m = &self.0.mcp;
        if m.kind == "command" {
            return self.run(&m.remove, &crate::config::bin_link());
        }
        json_remove_server(&expand(m.path.as_deref().unwrap_or("")), m.key.as_deref().unwrap_or(""))?;
        Ok(format!("Removed 'agentmux' from {}", m.path.as_deref().unwrap_or("")))
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let w = self.0.wake.as_ref()?;
        let bin = find_bin(&w.binary)?;
        let mut args: Vec<String> = w.args.clone();
        if !r.allow_edits {
            args.extend(w.read_only_args.clone());
        }
        let args = args
            .into_iter()
            .map(|a| a.replace("{prompt}", r.prompt).replace("{workspace}", r.workspace).replace("{ticket}", r.ticket))
            .collect();
        Some(WakeSpec { bin, args })
    }
}
