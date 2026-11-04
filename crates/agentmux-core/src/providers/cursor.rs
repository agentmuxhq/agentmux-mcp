use super::*;
use crate::wakers::find_bin;

pub struct Cursor;

fn config_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".cursor/mcp.json")
}

fn binary() -> Option<PathBuf> {
    find_bin("cursor-agent").or_else(|| find_bin("agent"))
}

impl Provider for Cursor {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "cursor".into(), name: "Cursor".into(), color: "#1C1C1C".into(), hint: "MCP stdio · ~/.cursor/mcp.json".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        let bin = binary();
        let app = config_path().parent().map(|p| p.exists()).unwrap_or(false);
        (app || bin.is_some(), bin.map(|b| b.display().to_string()).unwrap_or_else(|| "Cursor app found; cursor-agent CLI missing (needed to wake it)".into()))
    }
    fn configured(&self) -> bool {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| v["mcpServers"]["agentmux"].is_object())
            .unwrap_or(false)
    }
    fn install(&self, link: &Path) -> Result<String> {
        let entry = json!({"command": link.to_string_lossy(), "args": ["mcp", "--agent", "cursor"]});
        json_merge_server(&config_path(), "mcpServers.agentmux", entry, "Restart Cursor to load it.")
    }
    fn uninstall(&self) -> Result<String> {
        json_remove_server(&config_path(), "mcpServers.agentmux")?;
        Ok("Removed 'agentmux' from ~/.cursor/mcp.json".into())
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let s = |x: &str| x.to_string();
        let mut args = vec![s("-p"), s(r.prompt), s("--trust"), s("--workspace"), s(r.workspace), s("--output-format"), s("text")];
        if !r.allow_edits {
            args.extend([s("--mode"), s("ask")]);
        }
        Some(WakeSpec { bin: binary()?, args })
    }
}
