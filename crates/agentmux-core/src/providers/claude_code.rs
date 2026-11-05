use super::*;
use crate::wakers::{find_bin, spawn_path};
use std::process::Command;

/// Claude Code. Besides the MCP server it installs a context hook (see `hook.rs` in the CLI) that
/// tells Claude, each time the user sends a message, what other agents have answered meanwhile.
pub struct ClaudeCode;

fn binary() -> Option<PathBuf> {
    find_bin("claude")
}

fn run(args: &[&str]) -> Result<String> {
    let bin = binary().ok_or_else(|| anyhow::anyhow!("claude CLI not found"))?;
    let out = Command::new(&bin).args(args).env("PATH", spawn_path()).output()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("{} {} failed: {}", bin.display(), args.join(" "), text.trim());
    }
    Ok(text.trim().to_string())
}

/// `~/.claude/settings.json`; `AGENTMUX_CLAUDE_SETTINGS` overrides it (tests).
pub fn settings_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTMUX_CLAUDE_SETTINGS") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".claude/settings.json")
}

const EVENTS: &[&str] = &["UserPromptSubmit", "SessionStart"];

/// Claude Code's hook wiring.
pub const HOOK: chat_hook::HookSpec = chat_hook::HookSpec { agent: "claude", events: EVENTS, start_event: "SessionStart", mark: "hook claude-code", named: false };

pub use chat_hook::HookEvent;

/// Add (or, with `link = None`, remove) our entries in Claude Code's settings.
pub fn edit_hooks(path: &Path, link: Option<&Path>) -> Result<()> {
    chat_hook::edit_hooks(&HOOK, path, link)
}

pub fn hook_enabled() -> bool {
    chat_hook::hook_enabled(&HOOK, &settings_path())
}

/// Run by `agentmux hook claude-code`: remember the chat, return what to add to it.
pub fn on_hook(vault: &crate::Vault, ev: &HookEvent) -> Result<Option<String>> {
    chat_hook::on_hook(&HOOK, vault, ev)
}

/// What's new for Claude, as text for its chat.
pub fn hook_context(vault: &crate::Vault) -> Result<Option<String>> {
    chat_hook::updates_text(vault, "claude")
}

impl Provider for ClaudeCode {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "claude".into(), name: "Claude Code".into(), color: "#D97757".into(), hint: "MCP stdio · claude mcp add".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: true }
    }
    fn detect(&self) -> (bool, String) {
        match binary() {
            Some(b) => (true, b.display().to_string()),
            None => (false, "claude CLI not found".into()),
        }
    }
    /// Read `~/.claude.json` instead of `claude mcp list`: that command health-checks every MCP
    /// server by launching it, which is slow and would start AgentMux shims as a side effect.
    fn configured(&self) -> bool {
        let path = std::env::var("AGENTMUX_CLAUDE_JSON").map(PathBuf::from).unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".claude.json"));
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v["mcpServers"]["agentmux"].is_object())
    }
    fn install(&self, link: &Path) -> Result<String> {
        let l = link.to_string_lossy().to_string();
        let _ = run(&["mcp", "remove", "agentmux", "--scope", "user"]);
        run(&["mcp", "add", "--scope", "user", "agentmux", "--", &l, "mcp", "--agent", "claude"])?;
        let hook = match edit_hooks(&settings_path(), Some(link)) {
            Ok(()) => " The context hook is on: Claude is told what's new each time you send a message.",
            Err(_) => " (Could not add the context hook to ~/.claude/settings.json.)",
        };
        Ok(format!("Registered 'agentmux' with Claude Code (user scope). New Claude sessions will see it.{hook}"))
    }
    fn uninstall(&self) -> Result<String> {
        let _ = edit_hooks(&settings_path(), None);
        run(&["mcp", "remove", "agentmux", "--scope", "user"])
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let s = |x: &str| x.to_string();
        Some(WakeSpec {
            bin: binary()?,
            args: vec![s("-p"), s(r.prompt), s("--permission-mode"), s(if r.allow_edits { "acceptEdits" } else { "plan" })],
        })
    }
    fn set_option(&self, name: &str, on: bool) -> Result<String> {
        if name != "hook" {
            bail!("Claude Code has no option '{name}'");
        }
        if on {
            edit_hooks(&settings_path(), Some(&crate::config::bin_link()))?;
            Ok("Claude Code will be told what's new each time you send a message.".into())
        } else {
            edit_hooks(&settings_path(), None)?;
            Ok("Context hook removed from ~/.claude/settings.json.".into())
        }
    }
    fn options(&self) -> Value {
        json!({"hook": hook_enabled()})
    }
}

