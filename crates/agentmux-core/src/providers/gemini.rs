use super::*;
use crate::wakers::find_bin;

/// Gemini CLI. MCP servers live in `~/.gemini/settings.json` (`mcpServers`); a headless run is
/// `gemini -p`, read-only in plan mode. Like Claude Code it has chat hooks (`BeforeAgent` on each
/// message, `SessionStart`), so late replies reach the same chat and each chat keeps its thread.
pub struct Gemini;

/// Gemini CLI's hook wiring (same settings file as its MCP servers).
pub const HOOK: chat_hook::HookSpec = chat_hook::HookSpec {
    agent: "gemini",
    events: &["BeforeAgent", "SessionStart"],
    start_event: "SessionStart",
    mark: "hook gemini",
    named: true,
};

/// `~/.gemini/settings.json`; `GEMINI_CLI_HOME`-style override via `AGENTMUX_GEMINI_SETTINGS` (tests).
pub fn settings_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTMUX_GEMINI_SETTINGS") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".gemini/settings.json")
}

impl Provider for Gemini {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "gemini".into(), name: "Gemini CLI".into(), color: "#1A73E8".into(), hint: "MCP stdio · ~/.gemini/settings.json".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: true }
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
        let msg = json_merge_server(&settings_path(), "mcpServers.agentmux", entry, "New Gemini CLI sessions will see it.")?;
        let hook = match chat_hook::edit_hooks(&HOOK, &settings_path(), Some(link)) {
            Ok(()) => " The chat hook is on: Gemini is told what's new each time you send a message.",
            Err(_) => " (Could not add the chat hook.)",
        };
        Ok(format!("{msg}{hook}"))
    }
    fn uninstall(&self) -> Result<String> {
        let _ = chat_hook::edit_hooks(&HOOK, &settings_path(), None);
        json_remove_server(&settings_path(), "mcpServers.agentmux")?;
        Ok("Removed 'agentmux' and its chat hook from ~/.gemini/settings.json".into())
    }
    fn set_option(&self, name: &str, on: bool) -> Result<String> {
        if name != "hook" {
            bail!("Gemini CLI has no option '{name}'");
        }
        if on {
            chat_hook::edit_hooks(&HOOK, &settings_path(), Some(&crate::config::bin_link()))?;
            Ok("Gemini CLI will be told what's new each time you send a message.".into())
        } else {
            chat_hook::edit_hooks(&HOOK, &settings_path(), None)?;
            Ok("Chat hook removed from ~/.gemini/settings.json.".into())
        }
    }
    fn options(&self) -> Value {
        json!({"hook": chat_hook::hook_enabled(&HOOK, &settings_path())})
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let s = |x: &str| x.to_string();
        Some(WakeSpec {
            bin: find_bin("gemini")?,
            // --skip-trust: headless runs refuse untrusted folders, and an untrusted folder would also
            // silently turn plan (read-only) mode back into default. It trusts the folder for this run only.
            // --session-id: marked as AgentMux's run, so the chat hook (which can't see our env)
            // stays out of it.
            args: vec![
                s("-p"), s(r.prompt), s("--skip-trust"), s("--approval-mode"), s(if r.allow_edits { "auto_edit" } else { "plan" }),
                s("--output-format"), s("text"), s("--session-id"), chat_hook::wake_session_id(),
            ],
        })
    }
}
