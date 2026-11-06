//! Renaming an agent: announced to every agent that worked with it, told to the agent itself
//! (woken by webhook if it's a bot), unique names, and its @id address unchanged.

mod common;
use agentmux_core::webhook;
use common::*;
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn renaming_an_agent_is_announced_and_safe() {
    unsafe { std::env::set_var("AGENTMUX_SECRET_GROK_WEBHOOK_KEY_ANIMATOR", "k") };
    let (core, ws) = setup("rename-agent");
    let v = &core.vault;
    let bot = v.add_bot("Animator Grok Bot").unwrap();
    assert_eq!(bot, "grok:animator-grok-bot");
    // Webhook so the rename wakes it.
    unsafe { std::env::set_var("AGENTMUX_SECRET_GROK_WEBHOOK_KEY_ANIMATOR_GROK_BOT", "k") };
    let (url, seen) = fake_server(Arc::new(|_p: &str, _b: &Value| (200, json!({})))).await;
    webhook::save(v, &bot, &format!("{url}/automations/webhook/r1"), "k").unwrap();

    // The bot worked with Claude and Cursor in a thread.
    let t = core.start_ticket_in(&ctx("claude", &ws), None, None, &bot, "Export the frames?", None).unwrap();
    core.finish(&t, "answered", "Done", None).unwrap();
    let sess = session_of(&core, &t);
    let t2 = core.start_ticket_in(&ctx("cursor", &ws), None, Some(&sess), &bot, "And the sprites?", None).unwrap();
    core.finish(&t2, "answered", "Also done", None).unwrap();
    v.updates_for("claude", true).unwrap();
    v.updates_for("cursor", true).unwrap();

    // Names are unique.
    assert!(core.rename_agent(&bot, "claude code", "user").unwrap_err().to_string().contains("already the name of @claude"));

    // Rename: announced in the thread, to both agents, and to the bot (woken by webhook).
    assert_eq!(core.rename_agent(&bot, "Animation Director", "user").unwrap(), "Animation Director");
    assert_eq!(v.agent_name(&bot), "Animation Director");
    let msgs = v.messages(&sess).unwrap();
    assert!(msgs.iter().any(|m| m["type"] == "system" && m["text"].as_str().unwrap().contains("\"Animator Grok Bot\" is now \"Animation Director\"")));
    for a in ["claude", "cursor"] {
        let ups = v.updates_for(a, true).unwrap();
        assert!(ups.iter().any(|u| u["kind"] == "notice" && u["question"].as_str().unwrap().contains("Animation Director")), "{a}: {ups:?}");
    }
    let mine = v.updates_for(&bot, true).unwrap();
    assert!(mine.iter().any(|u| u["question"].as_str().unwrap().contains("You were renamed")), "{mine:?}");
    for _ in 0..50 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(seen.lock().unwrap().iter().any(|(_, b)| b.to_string().contains("You were renamed")), "bot woken with its new name");

    // Its address didn't change: asking @grok:animator-grok-bot still works.
    let t3 = core.start_ticket_in(&ctx("claude", &ws), None, Some(&sess), &bot, "Still there?", None).unwrap();
    assert_eq!(v.ticket(&t3).unwrap().unwrap()["to"], bot.as_str());
    core.finish(&t3, "answered", "Yes", None).unwrap();

    // A bot naming itself (mux_register) can't take another agent's name either.
    let (r, _) = tool(&core, &bot, &ws, "mux_register", json!({"name": "Cursor"})).await.unwrap_err().pipe_err();
    let _ = r;
}

trait PipeErr {
    fn pipe_err(self) -> (Value, Value);
}
impl PipeErr for String {
    fn pipe_err(self) -> (Value, Value) {
        assert!(self.contains("already the name of @cursor"), "{self}");
        (json!(null), json!(null))
    }
}
