use super::*;

/// Grok Bot: no headless CLI. Each bot (`grok`, `grok:<slug>`) is its own contact with its own
/// token, and is woken by a webhook (`webhook.rs`) or picks questions up through `mux_inbox`.
/// Over the tunnel it gets only the tools in `gateway::REMOTE_TOOLS` (+ `ASK_TOOLS` when allowed).
pub struct Grok;

impl Provider for Grok {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "grok".into(), name: "Grok Bot".into(), color: "#D97736".into(), hint: "MCP stdio · added in Grok Bot settings".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: true, remote_access: true, wake: "webhook", context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        (Path::new("/Applications/Grok Bot.app").exists(), "Grok Bot app (add AgentMux in its MCP settings)".into())
    }
    fn configured(&self) -> bool {
        false
    }
    fn install(&self, link: &Path) -> Result<String> {
        Ok(format!(
            "Grok Bot keeps MCP servers in its own settings, so add this by hand:\n  Grok Bot → Settings → MCP → Add server (stdio)\n  Command: {}\n  Args:    mcp --agent grok",
            link.display()
        ))
    }
    fn uninstall(&self) -> Result<String> {
        Ok("Remove the AgentMux server in that app's MCP settings.".into())
    }
}
