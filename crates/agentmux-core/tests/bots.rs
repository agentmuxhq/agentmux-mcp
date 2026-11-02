//! Bot contacts: slugs, removal, name resolution, self-registration.

use agentmux_core::{mcp, CallerCtx, Core, Vault};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

fn test_core(name: &str) -> (Arc<Core>, String) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("bots-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let vault = Arc::new(Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap());
    (Core::with_vault(vault), dir.join("ws").to_string_lossy().to_string())
}

fn ctx(agent: &str, cwd: &str) -> CallerCtx {
    CallerCtx { agent: agent.into(), cwd: cwd.into(), hop: 0 }
}

async fn tool(core: &Arc<Core>, who: &CallerCtx, name: &str, args: Value) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let resp = mcp::handle(core, who, req).await.unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("null");
    serde_json::from_str(text).unwrap_or(json!(text))
}

#[test]
fn add_and_remove_bots() {
    let (core, _) = test_core("add");
    let v = &core.vault;
    assert_eq!(v.add_bot("Job Hunter").unwrap(), "grok:job-hunter");
    assert_eq!(v.add_bot("job  hunter!").unwrap(), "grok:job-hunter-2", "collision gets a suffix");
    assert!(v.add_bot("  !!! ").is_err(), "nothing usable in the name");
    assert!(v.is_agent("grok:job-hunter") && v.is_agent("cursor") && !v.is_agent("grok:nope"));
    let p = v.providers().unwrap().into_iter().find(|p| p["id"] == "grok:job-hunter").unwrap();
    assert_eq!((p["family"].as_str(), p["wakeKind"].as_str(), p["name"].as_str()), (Some("grok"), Some("inbox"), Some("Job Hunter")));
    assert!(v.remove_bot("grok").is_err() && v.remove_bot("cursor").is_err(), "fixed agents stay");
    v.remove_bot("grok:job-hunter").unwrap();
    assert!(!v.is_agent("grok:job-hunter"));
}

#[tokio::test]
async fn names_resolve_to_one_agent() {
    let (core, ws) = test_core("resolve");
    // Only the original bot: plain "grok" works, and so does its @-form.
    assert_eq!(core.resolve_agent("grok").unwrap(), "grok");
    assert_eq!(core.resolve_agent("@Claude Code").unwrap(), "claude");
    core.vault.add_bot("Job Hunter").unwrap();
    core.vault.set_profile("grok", "AgentMux Manager", Some("runs the mux")).unwrap();
    let e = core.resolve_agent("grok").unwrap_err().to_string();
    assert!(e.contains("several Grok bots") && e.contains("grok (AgentMux Manager)") && e.contains("grok:job-hunter (Job Hunter)"), "{e}");
    assert_eq!(core.resolve_agent("grok:job-hunter").unwrap(), "grok:job-hunter");
    assert_eq!(core.resolve_agent("job-hunter").unwrap(), "grok:job-hunter");
    assert_eq!(core.resolve_agent("Job Hunter").unwrap(), "grok:job-hunter");
    assert_eq!(core.resolve_agent("agentmux manager").unwrap(), "grok");
    let e = core.resolve_agent("nobody").unwrap_err().to_string();
    assert!(e.contains("unknown agent") && e.contains("grok:job-hunter"), "{e}");
    // The same resolution applies to mux_ask_peer's `then`.
    let cursor = ctx("cursor", &ws);
    assert!(core.start_ticket(&cursor, None, "claude", "q", Some("nobody".into())).is_err());
}

#[tokio::test]
async fn bots_register_themselves() {
    let (core, ws) = test_core("register");
    let bot = core.vault.add_bot("Grok 2").unwrap();
    let me = ctx(&bot, &ws);
    let who = tool(&core, &me, "mux_whoami", json!({})).await;
    assert_eq!(who["agent"], bot.as_str());
    let before = core.vault.providers().unwrap();
    let reg0 = before.iter().find(|p| p["id"] == "grok").unwrap();
    assert_eq!(reg0["registered"], false, "the original bot hasn't introduced itself yet");

    let grok = ctx("grok", &ws);
    assert_eq!(tool(&core, &grok, "mux_whoami", json!({})).await["registered"], false);
    let r = tool(&core, &grok, "mux_register", json!({"name": "AgentMux Manager", "description": "keeps the mux healthy"})).await;
    assert_eq!(r["registered"], true);
    let who = tool(&core, &grok, "mux_whoami", json!({})).await;
    assert_eq!((who["name"].as_str(), who["registered"].as_bool(), who["description"].as_str()), (Some("AgentMux Manager"), Some(true), Some("keeps the mux healthy")));
    let contacts = tool(&core, &ctx("chatgpt", &ws), "mux_contacts", json!({})).await;
    let g = contacts.as_array().unwrap().iter().find(|c| c["id"] == "grok").unwrap();
    assert_eq!(g["name"], "AgentMux Manager");
    assert!(tool(&core, &grok, "mux_register", json!({"name": ""})).await.as_str().is_some(), "empty name is rejected");
}

#[test]
fn vault_backup_is_complete_and_pruned() {
    let (core, _) = test_core("backup");
    core.vault.add_bot("Job Hunter").unwrap();
    core.vault.set_setting("tunnel.cf_hostname", "mux.example.com").unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("bots-backup-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let first = core.vault.backup(&dir, "v0.1.0", 2).unwrap();
    // The copy opens on its own and has the same data.
    let copy = Vault::open_at(&first, &dir.join("threads")).unwrap();
    assert!(copy.is_agent("grok:job-hunter"));
    assert_eq!(copy.setting("tunnel.cf_hostname").unwrap().as_deref(), Some("mux.example.com"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // Only the newest `keep` backups survive.
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        core.vault.backup(&dir, "v0.1.0", 2).unwrap();
    }
    let n = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "db")).count();
    assert_eq!(n, 2);
}
