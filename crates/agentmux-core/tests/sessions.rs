//! A session is one conversation: unrelated exchanges in the same workspace get their own.

use agentmux_core::{AskOpts, CallerCtx, Core, Vault};
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

    // Going back to the first pair later continues where they left off: no timer, no new session.
    let d = core.start_ticket(&ctx("cursor", &ws), None, "grok", "later", None).unwrap();
    assert_eq!(session_of(&core, &d), s1);
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

#[test]
fn asking_outside_the_async_runtime_fails_cleanly() {
    // The desktop app once called this from a plain thread: the ticket was created but its wake never
    // started, leaving a question that stayed open forever. It must refuse before creating anything.
    let (core, ws) = test_core("noruntime");
    let err = core.start_ticket(&ctx("user", &ws), None, "claude", "hello?", None).unwrap_err().to_string();
    assert!(err.contains("async runtime"), "{err}");
    assert!(core.vault.open_tickets().unwrap().is_empty(), "no orphan ticket may be left behind");
}

#[tokio::test]
async fn orphaned_wake_tickets_are_swept_but_inbox_ones_are_kept() {
    let (core, ws) = test_core("sweep");
    unsafe { std::env::set_var("AGENTMUX_WAKE_TIMEOUT_SECS", "0") };
    let room_sess = core.vault.ensure_room(&ws, "main").unwrap();
    let (room, sess) = (room_sess.0, room_sess.1);
    // A CLI-woken agent whose wake never ran, and an inbox-only agent that simply hasn't looked yet.
    let stuck = core.vault.create_ticket(&sess, &room, "user", "claude", "anyone?", 1, None).unwrap();
    let inbox = core.vault.create_ticket(&sess, &room, "cursor", "grok", "whenever", 1, None).unwrap();
    assert_eq!(core.sweep_stale_after(-1), 1);
    assert_eq!(core.vault.ticket(&stuck).unwrap().unwrap()["status"], "expired");
    assert_eq!(core.vault.ticket(&inbox).unwrap().unwrap()["status"], "pending");
    // The pair is free again, so the user can ask once more.
    assert!(!core.vault.open_ticket_between("user", "claude").unwrap());
}

fn ask(core: &Arc<Core>, from: &str, ws: &str, to: &str, topic: Option<&str>, new_session: bool) -> String {
    let opts = AskOpts { topic: topic.map(String::from), new_session };
    let t = core.start_ticket_with(&ctx(from, ws), None, None, to, "q", None, &opts).unwrap();
    core.finish(&t, "answered", "ok", None).unwrap();
    session_of(core, &t)
}

fn title(core: &Core, session: &str) -> String {
    let room = core.vault.session_room(session).unwrap().unwrap();
    let r = core.vault.room(&room).unwrap().unwrap();
    r["sessions"].as_array().unwrap().iter().find(|x| x["id"] == session).unwrap()["title"].as_str().unwrap().to_string()
}

/// Sessions are topics: the same topic returns to its session (across days and agents), a new
/// topic gets its own, and nothing splits on a timer.
#[tokio::test]
async fn topics_route_to_their_sessions() {
    let (core, ws) = test_core("topics");

    // Claude asks Grok about the release: the room's empty first session takes that name.
    let release = ask(&core, "claude", &ws, "grok", Some("Release plan"), false);
    assert_eq!(title(&core, &release), "Release plan");

    // A QA question is a different conversation.
    let qa = ask(&core, "claude", &ws, "grok", Some("QA"), false);
    assert_ne!(qa, release);
    assert_eq!(title(&core, &qa), "QA");

    // Back to the release: same session, whatever the wording.
    assert_eq!(ask(&core, "claude", &ws, "grok", Some("the release plan"), false), release);
    assert_eq!(ask(&core, "claude", &ws, "grok", Some("Release Plan!"), false), release);

    // No topic: continue where this pair last talked (the release session).
    assert_eq!(ask(&core, "claude", &ws, "grok", None, false), release);

    // Another agent on the same topic joins the same conversation.
    assert_eq!(ask(&core, "cursor", &ws, "grok", Some("release plan"), false), release);

    // A topic that only shares a word is not the same topic.
    let notes = ask(&core, "claude", &ws, "grok", Some("release notes"), false);
    assert_ne!(notes, release);

    // Asked for explicitly: a fresh session, even on an existing topic.
    let fresh = ask(&core, "claude", &ws, "grok", Some("Release plan"), true);
    assert_ne!(fresh, release);

    // Renaming a session renames its topic too.
    core.vault.rename_session(&qa, "Quality checks").unwrap();
    assert_eq!(ask(&core, "claude", &ws, "grok", Some("quality checks"), false), qa);

    // Rooms can be named and addressed by name.
    let room = core.vault.session_room(&qa).unwrap().unwrap();
    core.vault.rename_room(&room, "AgentMux Discussion").unwrap();
    assert_eq!(core.vault.find_room("agentmux discussion").unwrap().as_deref(), Some(room.as_str()));
    let t = core.start_ticket(&ctx("claude", &ws), Some("AgentMux Discussion"), "grok", "by name", None).unwrap();
    assert_eq!(core.vault.ticket(&t).unwrap().unwrap()["roomId"], room.as_str());
    core.finish(&t, "answered", "ok", None).unwrap();

    // mux_sessions lists them by name with who took part.
    let list = core.vault.session_list(&room).unwrap();
    let names: Vec<&str> = list.iter().map(|x| x["name"].as_str().unwrap()).collect();
    for n in ["Release plan", "Quality checks", "release notes"] {
        assert!(names.contains(&n), "{names:?}");
    }
    assert!(list.iter().any(|x| x["participants"].as_array().unwrap().iter().any(|p| p == "cursor")));
}

#[test]
fn topic_matching() {
    use agentmux_core::vault::{topic_key, topics_match};
    assert_eq!(topic_key("The Release-Plan!"), "release plan");
    assert_eq!(topic_key("Session #3 · Release plan"), "release plan");
    assert!(topics_match("release plan", "release plan"));
    assert!(topics_match("release", "release plan"));
    assert!(!topics_match("release notes", "release plan"));
    assert!(!topics_match("", "release plan"));
}
