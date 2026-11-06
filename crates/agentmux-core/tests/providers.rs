//! The provider registry: every built-in is described, wake commands are unchanged by the
//! refactor, the Claude Code hook edits settings.json safely, and `mux_updates` tells each thing once.

use agentmux_core::providers::{self, claude_code, manifest::Manifest, WakeRequest};
use agentmux_core::{CallerCtx, Core, Vault};
use serde_json::{json, Value};
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("prov-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fake_bin(dir: &PathBuf, name: &str) {
    let p = dir.join(name);
    std::fs::write(&p, "#!/bin/sh\necho ok\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn registry_and_golden_wake_args() {
    let ids: Vec<String> = providers::builtin().iter().map(|p| p.info().id).collect();
    assert_eq!(ids, ["cursor", "grok", "claude", "chatgpt", "gemini", "claude-desktop"]);
    for p in providers::builtin() {
        let i = p.info();
        assert!(!i.name.is_empty() && i.color.starts_with('#'), "{i:?}");
        let _ = providers::describe(p.as_ref()); // detection never panics
    }
    assert!(providers::multi_instance("grok:job-hunter") && providers::remote_access("grok:x"));
    assert!(!providers::multi_instance("claude") && !providers::remote_access("cursor"));
    assert_eq!(providers::get("grok:job-hunter").unwrap().info().id, "grok");
    assert!(providers::get("claude:x").is_none(), "only multi-instance providers take a suffix");
    assert!(providers::get("claude").unwrap().caps().context_hook);

    let bins = tmp("bins");
    for b in ["cursor-agent", "claude", "codex", "gemini"] {
        fake_bin(&bins, b);
    }
    // SAFETY: this is the only test in this binary that touches the environment.
    unsafe { std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bins.display())) };
    let args = |id: &str, edits: bool| {
        let r = WakeRequest { prompt: "P", workspace: "/ws", allow_edits: edits, ticket: "T-1" };
        providers::get(id).unwrap().wake(&r).map(|w| w.args)
    };
    assert_eq!(args("cursor", false).unwrap(), ["-p", "P", "--trust", "--workspace", "/ws", "--output-format", "text", "--mode", "ask"]);
    assert_eq!(args("cursor", true).unwrap(), ["-p", "P", "--trust", "--workspace", "/ws", "--output-format", "text"]);
    assert_eq!(args("claude", false).unwrap(), ["-p", "P", "--permission-mode", "plan"]);
    assert_eq!(args("claude", true).unwrap(), ["-p", "P", "--permission-mode", "acceptEdits"]);
    assert_eq!(args("chatgpt", false).unwrap(), ["exec", "-s", "read-only", "--skip-git-repo-check", "--ephemeral", "-C", "/ws", "P"]);
    assert_eq!(args("chatgpt", true).unwrap()[2], "workspace-write");
    let g = args("gemini", false).unwrap();
    assert_eq!(g[..7], ["-p", "P", "--skip-trust", "--approval-mode", "plan", "--output-format", "text"]);
    assert_eq!(g[7], "--session-id");
    assert!(g[8].starts_with(providers::chat_hook::WAKE_SESSION_PREFIX) && g[8].len() == 36, "a UUID marked as AgentMux's: {}", g[8]);
    assert_eq!(args("gemini", true).unwrap()[4], "auto_edit");
    assert!(args("grok", false).is_none(), "Grok is woken by webhook, not a CLI");
}

#[test]
fn claude_hook_merges_and_removes_only_ours() {
    let d = tmp("hook");
    let path = d.join("settings.json");
    let theirs = json!({"model": "opus", "hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "~/bin/lint-prompt"}]}],
                                                  "Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}});
    std::fs::write(&path, theirs.to_string()).unwrap();
    let link = PathBuf::from("/Users/x/.agentmux/bin/agentmux");
    claude_code::edit_hooks(&path, Some(&link)).unwrap();
    claude_code::edit_hooks(&path, Some(&link)).unwrap(); // re-install doesn't duplicate
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["model"], "opus");
    let ups = v["hooks"]["UserPromptSubmit"].as_array().unwrap();
    assert_eq!(ups.len(), 2, "{ups:?}");
    assert_eq!(ups[1]["hooks"][0]["command"], "/Users/x/.agentmux/bin/agentmux hook claude-code");
    assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    assert!(d.join("settings.json.agentmux.bak").exists());

    claude_code::edit_hooks(&path, None).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["hooks"]["UserPromptSubmit"], theirs["hooks"]["UserPromptSubmit"]);
    assert_eq!(v["hooks"]["Stop"], theirs["hooks"]["Stop"]);
    assert!(v["hooks"].get("SessionStart").is_none());
}

#[tokio::test]
async fn updates_are_told_once() {
    let d = tmp("updates");
    std::fs::create_dir_all(d.join("ws")).unwrap();
    let vault = std::sync::Arc::new(Vault::open_at(&d.join("vault.db"), &d.join("threads")).unwrap());
    let core = Core::with_vault(vault.clone());
    let ws = d.join("ws").to_string_lossy().to_string();
    let ctx = |a: &str| CallerCtx { agent: a.into(), cwd: ws.clone(), hop: 0 };

    // Claude asks Grok (inbox-only here), Grok answers late; Grok asks Claude something.
    let t1 = core.start_ticket(&ctx("claude"), None, "grok", "is the build green?", None).unwrap();
    assert!(claude_code::hook_context(&vault).unwrap().is_none(), "nothing answered yet");
    core.finish(&t1, "expired", "timed out", None).unwrap();
    assert!(vault.updates_for("claude", false).unwrap().is_empty(), "expiry alone is not news");
    core.finish(&t1, "answered", "yes, all green", None).unwrap();
    let t2 = core.start_ticket(&ctx("grok"), None, "claude", "which branch?", None).unwrap();

    let peek = vault.updates_for("claude", false).unwrap();
    assert_eq!(peek.len(), 2, "{peek:?}");
    let text = claude_code::hook_context(&vault).unwrap().unwrap();
    assert!(text.contains("yes, all green") && text.contains(&t1), "{text}");
    assert!(text.contains(&t2) && text.contains("mux_reply"), "{text}");
    assert!(claude_code::hook_context(&vault).unwrap().is_none(), "told once");
    assert!(vault.updates_for("grok", true).unwrap().is_empty(), "grok's own question isn't news to grok");
}

#[test]
fn manifest_validation() {
    let ok = r##"
id = "aider"
name = "Aider"
detect = { binary = "aider" }
mcp = { kind = "json-merge", path = "~/.aider/mcp.json", key = "mcpServers.agentmux" }
wake = { binary = "aider", args = ["--message", "{prompt}", "--yes-always"], read_only_args = ["--dry-run"] }
"##;
    let m = Manifest::parse(ok).unwrap();
    assert_eq!(m.color, "#6B7280");
    let bad = |from: &str, to: &str| Manifest::parse(&ok.replace(from, to)).unwrap_err().to_string();
    assert!(bad("id = \"aider\"", "id = \"claude\"").contains("built-in"));
    assert!(bad("id = \"aider\"", "id = \"Ai der\"").contains("lowercase"));
    assert!(bad("{prompt}\", \"--yes", "{prompt}\", \"{home}\", \"--yes").contains("unknown placeholder"));
    assert!(bad("\"{prompt}\"", "\"hi\"").contains("{prompt}"));
    assert!(bad("json-merge", "shell").contains("unknown mcp.kind"));
}

#[test]
fn gemini_registers_in_its_settings_without_touching_the_rest() {
    let d = tmp("gemini");
    let path = d.join("settings.json");
    std::fs::write(&path, json!({"model": {"name": "gemini-3-pro"}, "mcpServers": {"pencil": {"command": "pencil"}}}).to_string()).unwrap();
    // SAFETY: only this test reads AGENTMUX_GEMINI_SETTINGS.
    unsafe { std::env::set_var("AGENTMUX_GEMINI_SETTINGS", &path) };
    let p = providers::get("gemini").unwrap();
    assert!(!p.configured());
    p.install(&PathBuf::from("/Users/x/.agentmux/bin/agentmux")).unwrap();
    assert!(p.configured());
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["agentmux"]["args"], json!(["mcp", "--agent", "gemini"]));
    assert_eq!(v["mcpServers"]["pencil"]["command"], "pencil");
    assert_eq!(v["model"]["name"], "gemini-3-pro");
    // Connecting also adds the chat hook (BeforeAgent + SessionStart), named, next to the user's own.
    let hooks = &v["hooks"];
    for ev in ["BeforeAgent", "SessionStart"] {
        let cmd = &hooks[ev][0]["hooks"][0];
        assert_eq!(cmd["command"], "/Users/x/.agentmux/bin/agentmux hook gemini", "{v}");
        assert_eq!(cmd["name"], "agentmux");
    }
    assert_eq!(p.options()["hook"], true);
    p.set_option("hook", false).unwrap();
    assert_eq!(p.options()["hook"], false);
    let v2: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(v2.get("hooks").is_none() && v2["mcpServers"]["agentmux"].is_object(), "hook off keeps the MCP server: {v2}");
    p.uninstall().unwrap();
    assert!(!p.configured());
    assert!(v["mcpServers"]["pencil"].is_object());
}

#[test]
fn cli_errors_lose_their_colour_codes() {
    assert_eq!(agentmux_core::wakers::strip_ansi("\u{1b}[31mGemini CLI is not running\u{1b}[0m here"), "Gemini CLI is not running here");
}

#[test]
fn claude_desktop_registers_in_the_app_config_and_waits_in_its_inbox() {
    let d = tmp("claude-desktop");
    let path = d.join("claude_desktop_config.json");
    std::fs::write(&path, json!({"mcpServers": {}, "preferences": {"theme": "dark"}, "coworkUserFilesPath": "/x"}).to_string()).unwrap();
    // SAFETY: only this test reads AGENTMUX_CLAUDE_DESKTOP_CONFIG.
    unsafe { std::env::set_var("AGENTMUX_CLAUDE_DESKTOP_CONFIG", &path) };
    let p = providers::get("claude-desktop").unwrap();
    assert_eq!(p.info().name, "Claude Desktop");
    assert_eq!(p.caps().wake, "inbox", "a chat can't be woken headlessly");
    assert!(p.wake(&WakeRequest { prompt: "hi", workspace: "/tmp", allow_edits: false, ticket: "T-1" }).is_none());
    assert!(!p.configured());
    p.install(&PathBuf::from("/Users/x/.agentmux/bin/agentmux")).unwrap();
    assert!(p.configured());
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["agentmux"]["args"], json!(["mcp", "--agent", "claude-desktop"]));
    assert_eq!(v["preferences"]["theme"], "dark", "the app's own settings are kept");
    assert_eq!(v["coworkUserFilesPath"], "/x");
    p.uninstall().unwrap();
    assert!(!p.configured());
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["preferences"]["theme"], "dark");
}
