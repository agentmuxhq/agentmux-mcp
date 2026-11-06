//! A cloud bot (its requests come from AgentMux's own folder) asking back the agent it's working
//! with stays in their conversation: the same product, the same thread, never a room named after
//! a folder. Reproduces the Odrisso Suto handoff that landed in a new ".agentmux" room.

mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn a_bot_asking_back_stays_in_the_conversation() {
    let (core, ws) = setup("bot-threads");
    let home = ws.parent().unwrap().join("agentmux-home");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: only test in this binary. Bots' requests carry AgentMux's own folder as their cwd.
    unsafe { std::env::set_var("AGENTMUX_HOME", &home) };
    let v = &core.vault;
    let bot = v.add_bot("Animator Grok Bot").unwrap();
    let root = std::path::PathBuf::from("/"); // Claude Desktop runs from /

    // Claude Desktop hands the reel off to the bot, in the Odrisso Suto product.
    let (r, _) = tool(&core, "claude-desktop", &root, "mux_ask_peer", json!({"to": bot, "room": "Odrisso Suto", "topic": "Odrisso Suto reel handoff",
        "question": "Here's the Odrisso Suto handoff: story, rigs and sheets.", "wait_s": 0})).await.unwrap();
    let t1 = r["ticket"].as_str().unwrap_or_else(|| panic!("{r}")).to_string();
    let handoff = session_of(&core, &t1);
    let product = v.ticket(&t1).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();
    core.finish(&t1, "answered", "Received and forwarded.", None).unwrap();

    // The bot asks back with its own topic: same product, same thread (it can't ask the user).
    let (r, _) = tool(&core, &bot, &home, "mux_ask_peer", json!({"to": "claude-desktop", "topic": "Odrisso Suto script",
        "question": "Please export the full Bangla dialogue script.", "wait_s": 0})).await.unwrap();
    let t2 = r["ticket"].as_str().unwrap_or_else(|| panic!("{r}")).to_string();
    assert_eq!(v.ticket(&t2).unwrap().unwrap()["roomId"], product.as_str(), "{r}");
    assert_eq!(session_of(&core, &t2), handoff);
    core.finish(&t2, "answered", "Saved script.md", None).unwrap();

    // Without a topic, too.
    let (r, _) = tool(&core, &bot, &home, "mux_ask_peer", json!({"to": "claude-desktop", "question": "Which scene first?", "wait_s": 0})).await.unwrap();
    let t3 = r["ticket"].as_str().unwrap().to_string();
    assert_eq!(session_of(&core, &t3), handoff);
    core.finish(&t3, "answered", "Scene 1", None).unwrap();

    // A fresh thread when the bot asks for one: still the same product.
    let (r, _) = tool(&core, &bot, &home, "mux_ask_peer", json!({"to": "claude-desktop", "question": "New topic: voice casting?", "new_session": true, "wait_s": 0})).await.unwrap();
    let t4 = r["ticket"].as_str().unwrap().to_string();
    assert_eq!(v.ticket(&t4).unwrap().unwrap()["roomId"], product.as_str());
    assert_ne!(session_of(&core, &t4), handoff);
    core.finish(&t4, "answered", "ok", None).unwrap();

    // Claude Desktop is inbox-only: the bot can queue follow-ups while one is still waiting
    // (it reads them all next time it's used); a woken agent still takes one at a time.
    let (r, _) = tool(&core, &bot, &home, "mux_ask_peer", json!({"to": "claude-desktop", "question": "Script still needed.", "wait_s": 0})).await.unwrap();
    assert!(r["ticket"].is_string(), "{r}");
    let (r, _) = tool(&core, &bot, &home, "mux_ask_peer", json!({"to": "claude-desktop", "question": "Director asks: does make.py render all shots?", "wait_s": 0})).await.unwrap();
    assert!(r["ticket"].is_string(), "second question queues behind the first: {r}");
    assert_eq!(v.open_tickets_between(&bot, "claude-desktop").unwrap(), 2);
    for i in 0..8 {
        tool(&core, &bot, &home, "mux_ask_peer", json!({"to": "claude-desktop", "question": format!("q{i}"), "wait_s": 0})).await.unwrap();
    }
    let err = tool(&core, &bot, &home, "mux_ask_peer", json!({"to": "claude-desktop", "question": "one too many", "wait_s": 0})).await.unwrap_err();
    assert!(err.contains("10 questions waiting"), "{err}");
    let t = core.start_ticket_in(&ctx(&bot, &home), None, None, "claude", "Woken agent?", None).unwrap();
    assert!(core.start_ticket_in(&ctx(&bot, &home), None, None, "claude", "Again?", None).unwrap_err().to_string().contains("already has an open question"));
    core.finish(&t, "answered", "ok", None).unwrap();

    // A bot with no history with that agent: the Inbox, never a room named after AgentMux's folder.
    let other = v.add_bot("Job Hunter").unwrap();
    let (r, _) = tool(&core, &other, &home, "mux_ask_peer", json!({"to": "claude", "question": "Any openings?", "wait_s": 0})).await.unwrap();
    let t5 = r["ticket"].as_str().unwrap().to_string();
    let room = v.ticket(&t5).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();
    assert_eq!(v.room(&room).unwrap().unwrap()["title"], "Inbox");
    let names: Vec<String> = v.room_ids().unwrap().iter().filter_map(|id| v.room(id).unwrap().map(|r| r["title"].as_str().unwrap().to_string())).collect();
    assert!(!names.iter().any(|n| n == "agentmux-home" || n == ".agentmux" || n == "/"), "{names:?}");
}
