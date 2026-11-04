//! `agentmux hook claude-code`: run by Claude Code on every prompt (UserPromptSubmit) and at
//! session start. Prints what other agents answered since Claude last looked, which Claude Code
//! adds to the conversation as context. It must never break the user's prompt, so every failure
//! is silent and it always exits 0.

use agentmux_core::{config, connectors, providers::claude_code, Vault};
use anyhow::{bail, Result};
use std::io::Read;

pub fn run(target: &str, on: bool, off: bool) -> Result<()> {
    if target != "claude-code" && target != "claude" {
        bail!("unknown hook '{target}' (available: claude-code)");
    }
    if on || off {
        println!("{}", connectors::set_option("claude", "hook", on)?);
        return Ok(());
    }
    // Claude Code sends the event as JSON on stdin; we don't need it, but drain it.
    let mut buf = String::new();
    let _ = std::io::stdin().take(1 << 20).read_to_string(&mut buf);
    // Headless runs that AgentMux itself started must not consume the user's updates.
    if std::env::var_os("AGENTMUX_TICKET").is_some() || !config::db_path().exists() {
        return Ok(());
    }
    if let Ok(vault) = Vault::open() {
        let ev = claude_code::HookEvent::parse(&buf);
        if let Ok(Some(text)) = claude_code::on_hook(&vault, &ev) {
            println!("{text}");
        }
    }
    Ok(())
}
