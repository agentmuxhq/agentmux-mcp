//! Provider plugins. One file per agent; `all()` is the registry the rest of the core iterates.
//!
//! To add a provider: implement [`Provider`] in a new file here, add it to [`builtin`], and add a
//! test. No other core file needs a `match agent` arm. For simple CLI agents, a TOML manifest in
//! `~/.agentmux/providers/` does the same without any Rust (see `manifest.rs`).

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod grok;
pub mod manifest;

/// How a provider shows up in the UI and in `mux_contacts`.
#[derive(Clone, Debug)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub color: String,
    /// One line on how it connects, e.g. "MCP stdio · ~/.cursor/mcp.json".
    pub hint: String,
}

/// What a provider can do. The core and the UI branch on these, never on provider ids.
#[derive(Clone, Debug)]
pub struct Capabilities {
    /// Many named instances (`grok:<slug>`), each with its own identity.
    pub multi_instance: bool,
    /// Reachable over the HTTP gateway with a bearer token and a small tool set.
    pub remote_access: bool,
    /// How a question wakes it: "cli" (headless run), "webhook" or "inbox" (it polls `mux_inbox`).
    pub wake: &'static str,
    /// Has a context hook that injects AgentMux updates into its own chat.
    pub context_hook: bool,
}

pub struct WakeRequest<'a> {
    pub prompt: &'a str,
    pub workspace: &'a str,
    pub allow_edits: bool,
    pub ticket: &'a str,
}

/// A headless command that answers a ticket on stdout.
pub struct WakeSpec {
    pub bin: PathBuf,
    pub args: Vec<String>,
}

pub trait Provider: Send + Sync {
    fn info(&self) -> ProviderInfo;
    fn caps(&self) -> Capabilities;
    /// (installed, detail) for the connect screen.
    fn detect(&self) -> (bool, String);
    /// Is AgentMux registered as an MCP server in this agent?
    fn configured(&self) -> bool;
    /// Register AgentMux. `link` is the stable `~/.agentmux/bin/agentmux` path.
    fn install(&self, link: &Path) -> Result<String>;
    fn uninstall(&self) -> Result<String>;
    /// The headless command to wake it, or None if it is woken another way.
    fn wake(&self, _req: &WakeRequest) -> Option<WakeSpec> {
        None
    }
    /// Provider-specific switches, e.g. `("hook", true)`. Returns a status line.
    fn set_option(&self, name: &str, _on: bool) -> Result<String> {
        bail!("{} has no option '{name}'", self.info().name)
    }
    /// Current values of the switches `set_option` accepts.
    fn options(&self) -> Value {
        json!({})
    }
}

pub fn builtin() -> Vec<Arc<dyn Provider>> {
    vec![Arc::new(cursor::Cursor), Arc::new(grok::Grok), Arc::new(claude_code::ClaudeCode), Arc::new(codex::Codex)]
}

/// Built-ins first, then manifests from `~/.agentmux/providers/`.
pub fn all() -> Vec<Arc<dyn Provider>> {
    let mut v = builtin();
    for m in manifest::load_all() {
        if !v.iter().any(|p| p.info().id == m.info().id) {
            v.push(m);
        }
    }
    v
}

/// The provider for an id; a `grok:<slug>` bot resolves to `grok`.
pub fn get(id: &str) -> Option<Arc<dyn Provider>> {
    let base = id.split(':').next().unwrap_or(id);
    all().into_iter().find(|p| {
        let pid = p.info().id;
        pid == id || (pid == base && p.caps().multi_instance)
    })
}

pub fn is_known(id: &str) -> bool {
    get(id).is_some_and(|p| p.info().id == id)
}

pub fn display_name(id: &str) -> String {
    get(id).map(|p| p.info().name).unwrap_or_else(|| id.to_string())
}

pub fn multi_instance(id: &str) -> bool {
    get(id).is_some_and(|p| p.caps().multi_instance)
}

pub fn remote_access(id: &str) -> bool {
    get(id).is_some_and(|p| p.caps().remote_access)
}

/// UI/CLI view of every provider: info, capabilities and live detection.
pub fn describe(p: &dyn Provider) -> Value {
    let i = p.info();
    let c = p.caps();
    let (installed, detail) = p.detect();
    json!({
        "agent": i.id, "name": i.name, "color": i.color, "hint": i.hint,
        "installed": installed, "detail": detail, "configured": p.configured(),
        "multiInstance": c.multi_instance, "remoteAccess": c.remote_access,
        "wake": c.wake, "contextHook": c.context_hook, "options": p.options(),
    })
}

/// Static view (no shelling out) used where detection would be too slow.
pub fn catalog() -> Vec<Value> {
    all()
        .iter()
        .map(|p| {
            let i = p.info();
            let c = p.caps();
            json!({"id": i.id, "name": i.name, "color": i.color, "hint": i.hint,
                   "multiInstance": c.multi_instance, "remoteAccess": c.remote_access,
                   "wake": c.wake, "contextHook": c.context_hook})
        })
        .collect()
}

pub(crate) fn json_merge_server(path: &Path, key_path: &str, entry: Value, what: &str) -> Result<String> {
    use crate::vault::atomic_write;
    use anyhow::Context;
    let mut cfg: Value = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        let bak = path.with_extension("json.agentmux.bak");
        if !bak.exists() {
            atomic_write(&bak, &raw)?; // 0600; the original may contain tokens
        }
        serde_json::from_str(&raw).with_context(|| format!("{} is not valid JSON; refusing to edit", path.display()))?
    } else {
        std::fs::create_dir_all(path.parent().unwrap())?;
        json!({})
    };
    let mut cur = &mut cfg;
    for k in key_path.split('.') {
        if !cur[k].is_object() {
            cur[k] = json!({});
        }
        cur = &mut cur[k];
    }
    *cur = entry;
    atomic_write(path, &serde_json::to_string_pretty(&cfg)?)?;
    Ok(format!("Added 'agentmux' to {} (backup: *.agentmux.bak). {what}", path.display()))
}

pub(crate) fn json_remove_server(path: &Path, key_path: &str) -> Result<()> {
    let mut cfg: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let mut parts: Vec<&str> = key_path.split('.').collect();
    let last = parts.pop().unwrap_or("agentmux");
    let mut cur = &mut cfg;
    for k in parts {
        cur = &mut cur[k];
    }
    if let Some(m) = cur.as_object_mut() {
        m.remove(last);
    }
    crate::vault::atomic_write(path, &serde_json::to_string_pretty(&cfg)?)
}
