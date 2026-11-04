use super::*;
use crate::wakers::find_bin;

/// Gemini CLI. MCP servers live in `~/.gemini/settings.json` (`mcpServers`); a headless run is
/// `gemini -p`, read-only in plan mode.
pub struct Gemini;

/// `~/.gemini/settings.json`; `GEMINI_CLI_HOME`-style override via `AGENTMUX_GEMINI_SETTINGS` (tests).
fn settings_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTMUX_GEMINI_SETTINGS") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".gemini/settings.json")
}

impl Provider for Gemini {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "gemini".into(), name: "Gemini CLI".into(), color: "#4285F4".into(), hint: "MCP stdio · ~/.gemini/settings.json".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        match find_bin("gemini") {
            Some(b) => (true, b.display().to_string()),
            None => (false, "gemini CLI not found (npm install -g @google/gemini-cli)".into()),
        }
    }
    fn configured(&self) -> bool {
        std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v["mcpServers"]["agentmux"].is_object())
    }
    fn install(&self, link: &Path) -> Result<String> {
        let entry = json!({"command": link.to_string_lossy(), "args": ["mcp", "--agent", "gemini"]});
        json_merge_server(&settings_path(), "mcpServers.agentmux", entry, "New Gemini CLI sessions will see it.")
    }
    fn uninstall(&self) -> Result<String> {
        json_remove_server(&settings_path(), "mcpServers.agentmux")?;
        Ok("Removed 'agentmux' from ~/.gemini/settings.json".into())
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let s = |x: &str| x.to_string();
        Some(WakeSpec {
            bin: find_bin("gemini")?,
            args: vec![s("-p"), s(r.prompt), s("--approval-mode"), s(if r.allow_edits { "auto_edit" } else { "plan" }), s("--output-format"), s("text")],
        })
    }
}
