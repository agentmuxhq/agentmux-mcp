//! The hook commands' output formats: Gemini CLI parses stdout as JSON (so even "nothing new" must
//! be `{}`), Claude Code takes plain text (nothing at all when nothing is new).

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_agentmux");

fn run(home: &std::path::Path, target: &str, stdin: &str, extra: &[(&str, &str)]) -> String {
    let mut c = Command::new(BIN);
    c.args(["hook", target]).env("AGENTMUX_HOME", home).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, v) in extra {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "hooks always exit 0");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn hook_output_formats() {
    let home = std::env::temp_dir().join(format!("agentmux-hookout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let ev = format!(r#"{{"hook_event_name":"BeforeAgent","session_id":"g1","cwd":"{}"}}"#, home.display());

    // No vault yet: Gemini still gets valid JSON, Claude gets nothing.
    assert_eq!(run(&home, "gemini", &ev, &[]).trim(), "{}");
    assert_eq!(run(&home, "claude-code", &ev, &[]).trim(), "");

    // Something new for Gemini: a question addressed to it.
    let vault = agentmux_core::Vault::open_at(&home.join("vault.db"), &home.join("threads")).unwrap();
    let (room, sess) = vault.ensure_room(&home.to_string_lossy(), "").unwrap();
    vault.create_ticket(&sess, &room, "claude", "gemini", "Check the release notes?", 1, None).unwrap();
    let out = run(&home, "gemini", &ev, &[]);
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("stdout is JSON");
    let ctx = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(ctx.contains("Check the release notes?"), "{ctx}");
    // Told once.
    assert_eq!(run(&home, "gemini", &ev, &[]).trim(), "{}");
    // Inside AgentMux's own headless wake: never consume the user's updates.
    vault.create_ticket(&sess, &room, "claude", "gemini", "Another one", 1, None).unwrap();
    assert_eq!(run(&home, "gemini", &ev, &[("AGENTMUX_TICKET", "T-x")]).trim(), "{}");
    assert!(run(&home, "gemini", &ev, &[]).contains("Another one"));
    // Unknown targets fail loudly (that's a setup mistake, not a chat).
    assert!(!Command::new(BIN).args(["hook", "nope"]).env("AGENTMUX_HOME", &home).stdin(Stdio::null()).output().unwrap().status.success());
}
