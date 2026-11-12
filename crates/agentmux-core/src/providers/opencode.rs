//! OpenCode (opencode.ai): a terminal coding agent. Connecting adds AgentMux as a local MCP
//! server in its global config; questions wake it headlessly with `opencode run`.

use super::*;
use crate::wakers::find_bin;

pub struct OpenCode;

/// `~/.config/opencode/opencode.json` (`AGENTMUX_OPENCODE_CONFIG` overrides it, for tests).
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTMUX_OPENCODE_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".config/opencode/opencode.json")
}

fn binary() -> Option<PathBuf> {
    find_bin("opencode")
}

fn read() -> Option<Value> {
    std::fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// Where AgentMux's entry goes. Current OpenCode keeps servers under `mcp.servers`; older
/// versions read `mcp.<name>` directly. Follow whichever the user's file already uses.
fn key_path(cfg: &Option<Value>) -> &'static str {
    let mcp = cfg.as_ref().map(|c| &c["mcp"]);
    let legacy = mcp.and_then(|m| m.as_object()).is_some_and(|m| !m.contains_key("servers") && m.values().any(|v| v.get("type").is_some()));
    if legacy { "mcp.agentmux" } else { "mcp.servers.agentmux" }
}

impl Provider for OpenCode {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "opencode".into(), name: "OpenCode".into(), color: "#211E1E".into(), hint: "MCP stdio · ~/.config/opencode/opencode.json".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        match binary() {
            Some(b) => (true, b.display().to_string()),
            None => (config_path().exists(), "opencode CLI not found (install it from opencode.ai; needed to wake it)".into()),
        }
    }
    fn configured(&self) -> bool {
        read().is_some_and(|v| v["mcp"]["servers"]["agentmux"].is_object() || v["mcp"]["agentmux"].is_object())
    }
    fn install(&self, link: &Path) -> Result<String> {
        let cfg = read();
        let path = key_path(&cfg);
        let mut entry = json!({"type": "local", "command": [link.to_string_lossy(), "mcp", "--agent", "opencode"]});
        if path == "mcp.agentmux" {
            entry["enabled"] = json!(true); // the older format's switch
        }
        json_merge_server(&config_path(), path, entry, "Restart OpenCode to load it.")
    }
    fn uninstall(&self) -> Result<String> {
        // Only touch keys that exist, so an older-format file never gains an empty `servers`.
        if let Some(v) = read() {
            if v["mcp"]["servers"]["agentmux"].is_object() {
                json_remove_server(&config_path(), "mcp.servers.agentmux")?;
            }
            if v["mcp"]["agentmux"].is_object() {
                json_remove_server(&config_path(), "mcp.agentmux")?;
            }
        }
        Ok(format!("Removed 'agentmux' from {}", config_path().display()))
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let s = |x: &str| x.to_string();
        // `plan` is OpenCode's read-only agent, `build` can edit. --auto: nobody is there to approve
        // prompts (plan still refuses edits). --archived keeps these runs out of the user's sessions.
        let agent = if r.allow_edits { "build" } else { "plan" };
        let args = vec![s("run"), s("--agent"), s(agent), s("--auto"), s("--archived"), s("--title"), format!("AgentMux {}", r.ticket), s(r.prompt)];
        Some(WakeSpec { bin: binary()?, args })
    }
}
