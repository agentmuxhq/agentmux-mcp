//! Claude Desktop: the Claude app's chats (not Claude Code). It loads local MCP servers from
//! `claude_desktop_config.json`, so connecting is one entry there. Nothing can start a chat in
//! the background, so questions to it wait in its inbox and show up when you next talk to it
//! (it calls `mux_updates` / `mux_inbox` at the start of a turn).

use super::*;

pub struct ClaudeDesktop;

/// `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS
/// (`AGENTMUX_CLAUDE_DESKTOP_CONFIG` overrides it, for tests).
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTMUX_CLAUDE_DESKTOP_CONFIG") {
        return PathBuf::from(p);
    }
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir().unwrap_or_default().join("Library/Application Support")
    } else {
        dirs::config_dir().unwrap_or_default()
    };
    base.join("Claude/claude_desktop_config.json")
}

impl Provider for ClaudeDesktop {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "claude-desktop".into(),
            name: "Claude Desktop".into(),
            color: "#D97757".into(),
            hint: "MCP stdio · Claude app chats (claude_desktop_config.json)".into(),
        }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "inbox", context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        let app = std::path::Path::new("/Applications/Claude.app").exists() || config_path().parent().map(|p| p.exists()).unwrap_or(false);
        let detail = if app {
            format!("{} · questions wait until you next message it", config_path().display())
        } else {
            "Claude app not found".into()
        };
        (app, detail)
    }
    fn configured(&self) -> bool {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| v["mcpServers"]["agentmux"].is_object())
            .unwrap_or(false)
    }
    fn install(&self, link: &Path) -> Result<String> {
        let entry = json!({"command": link.to_string_lossy(), "args": ["mcp", "--agent", "claude-desktop"]});
        json_merge_server(&config_path(), "mcpServers.agentmux", entry, "Quit and reopen the Claude app to load it.")
    }
    fn uninstall(&self) -> Result<String> {
        json_remove_server(&config_path(), "mcpServers.agentmux")?;
        Ok(format!("Removed 'agentmux' from {}", config_path().display()))
    }
    fn wake(&self, _r: &WakeRequest) -> Option<WakeSpec> {
        None // a chat can't be started headlessly; it reads its inbox on your next message
    }
}
