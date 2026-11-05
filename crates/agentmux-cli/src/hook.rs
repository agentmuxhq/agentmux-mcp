//! `agentmux hook claude-code` / `agentmux hook gemini`: run by the chat client on every message
//! and when a chat starts. Prints what other agents answered since the agent last looked (and, on
//! chat start, which AgentMux thread the chat is on), which the client adds to the conversation.
//! It must never break the user's prompt, so every failure is silent and it always exits 0.
//!
//! Claude Code takes plain text on stdout. Gemini CLI takes JSON only:
//! `{"hookSpecificOutput": {"additionalContext": "…"}}`, or `{}` for nothing.

use agentmux_core::providers::{chat_hook, claude_code, gemini};
use agentmux_core::{config, connectors, Vault};
use anyhow::{bail, Result};
use serde_json::json;
use std::io::Read;

pub fn run(target: &str, on: bool, off: bool) -> Result<()> {
    let (agent, spec, json_out) = match target {
        "claude-code" | "claude" => ("claude", &claude_code::HOOK, false),
        "gemini" => ("gemini", &gemini::HOOK, true),
        _ => bail!("unknown hook '{target}' (available: claude-code, gemini)"),
    };
    if on || off {
        println!("{}", connectors::set_option(agent, "hook", on)?);
        return Ok(());
    }
    let mut buf = String::new();
    let _ = std::io::stdin().take(1 << 20).read_to_string(&mut buf);
    let text = context(spec, &buf);
    if json_out {
        let out = match text {
            Some(t) => json!({"hookSpecificOutput": {"additionalContext": t}}),
            None => json!({}),
        };
        println!("{out}");
    } else if let Some(t) = text {
        println!("{t}");
    }
    Ok(())
}

fn context(spec: &chat_hook::HookSpec, stdin: &str) -> Option<String> {
    // Headless runs that AgentMux itself started must not consume the user's updates.
    if std::env::var_os("AGENTMUX_TICKET").is_some() || !config::db_path().exists() {
        return None;
    }
    let vault = Vault::open().ok()?;
    chat_hook::on_hook(spec, &vault, &chat_hook::HookEvent::parse(stdin)).ok().flatten()
}
