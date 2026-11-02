//! A session is one conversation: unrelated exchanges in the same workspace get their own.

use agentmux_core::{CallerCtx, Core, Vault};
use std::path::PathBuf;
use std::sync::Arc;

fn test_core(name: &str) -> (Arc<Core>, String) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sess-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let vault = Arc::new(Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap());
    (Core::with_vault(vault), dir.join("ws").to_string_lossy().to_string())
}

fn ctx(agent: &str, cwd: &str) -> CallerCtx {
    CallerCtx { agent: agent.into(), cwd: cwd.into(), hop: 0 }
}

fn session_of(core: &Core, ticket: &str) -> String {
    core.vault.ticket(ticket).unwrap().unwrap()["sessionId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn sessions_follow_conversations() {
    let (core, ws) = test_core("pairs");
    // Grok is inbox-only here, so nothing is actually woken.
    let a = core.start_ticket(&ctx("cursor", &ws), None, "grok", "first", None).unwrap();
    let s1 = session_of(&core, &a);
    core.finish(&a, "answered", "ok", None).unwrap();

    // Same pair again (either direction) continues the conversation.
    let b = core.start_ticket(&ctx("grok", &ws), None, "cursor", "follow-up", None).unwrap();
    assert_eq!(session_of(&core, &b), s1);
    core.finish(&b, "answered", "ok", None).unwrap();

    // A different pair in the same workspace starts its own session.
    let c = core.start_ticket(&ctx("claude", &ws), None, "grok", "unrelated", None).unwrap();
    let s2 = session_of(&core, &c);
    assert_ne!(s2, s1, "claude↔grok must not land in cursor↔grok's session");
    let room = core.vault.ticket(&c).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();
    let r = core.vault.room(&room).unwrap().unwrap();
    let title = r["sessions"].as_array().unwrap().iter().find(|x| x["id"] == s2.as_str()).unwrap()["title"].as_str().unwrap().to_string();
    assert!(title.contains("Claude Code ↔ Grok Bot"), "{title}");
    core.finish(&c, "answered", "ok", None).unwrap();

    // Going back to the first pair now that it isn't the latest session: new session.
    let d = core.start_ticket(&ctx("cursor", &ws), None, "grok", "later", None).unwrap();
    assert_ne!(session_of(&core, &d), s1);
    assert_ne!(session_of(&core, &d), s2);
    core.finish(&d, "answered", "ok", None).unwrap();

    // ...unless the caller asks to continue it.
    let e = core.start_ticket_in(&ctx("cursor", &ws), None, Some(&s1), "grok", "explicit", None).unwrap();
    assert_eq!(session_of(&core, &e), s1);
    core.finish(&e, "answered", "ok", None).unwrap();
    assert!(core.start_ticket_in(&ctx("cursor", &ws), None, Some("sess-nope"), "grok", "x", None).is_err());
}

#[tokio::test]
async fn empty_session_is_reused_and_handoffs_stay_put() {
    let (core, ws) = test_core("empty");
    let first = core.start_ticket(&ctx("cursor", &ws), None, "grok", "hi", None).unwrap();
    core.finish(&first, "answered", "ok", None).unwrap();
    let room = core.vault.ticket(&first).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();

    // "New session" in the UI makes an empty session; the next ask goes there whoever it's from.
    let fresh = core.vault.new_session(&room, "").unwrap();
    let t = core.start_ticket(&ctx("claude", &ws), None, "grok", "go", Some("cursor".into())).unwrap();
    assert_eq!(session_of(&core, &t), fresh);

    // The `then` handoff (grok → cursor) stays in the same conversation.
    core.finish(&t, "answered", "grok's answer", None).unwrap();
    let inbox = core.vault.inbox("cursor").unwrap();
    let handoff = inbox.iter().find(|x| x["from"] == "grok").expect("handoff ticket to cursor");
    assert_eq!(session_of(&core, handoff["ticket"].as_str().unwrap()), fresh);
}
