//! A Grok bot asks Claude Desktop (inbox-only) and its run ends. Hours later Claude Desktop
//! answers: the bot is woken by webhook with the answer, so it can pass it on. A bot that got the
//! answer while still waiting in the same run isn't woken again.

mod common;
use agentmux_core::webhook;
use common::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn an_answer_wakes_the_bot_that_asked() {
    let (core, ws) = setup("answer-wakes-bot");
    let v = &core.vault;
    let bot = v.add_bot("Animator").unwrap();
    // SAFETY: only test in this binary; the webhook key lives in this env var instead of the Keychain.
    unsafe { std::env::set_var("AGENTMUX_SECRET_GROK_WEBHOOK_KEY_ANIMATOR", "k") };
    let (url, seen) = fake_server(Arc::new(|_p: &str, _b: &Value| (200, json!({})))).await;
    webhook::save(v, &bot, &format!("{url}/automations/webhook/r1"), "k").unwrap();
    let posts = |needle: &str| seen.lock().unwrap().iter().filter(|(_, b)| b.to_string().contains(needle)).count();
    let root = std::path::PathBuf::from("/");

    // The bot asks and leaves (wait_s 0, its run ends).
    let (r, _) = tool(&core, &bot, &ws, "mux_ask_peer", json!({"to": "claude-desktop", "question": "Export the Bangla script?", "wait_s": 0})).await.unwrap();
    let t = r["ticket"].as_str().unwrap().to_string();
    assert!(r["waiting_on_user"].is_string(), "{r}");

    // Later, Claude Desktop answers from its chat.
    tool(&core, "claude-desktop", &root, "mux_reply", json!({"ticket": t, "text": "Here are the confirmed lines and the scene order."})).await.unwrap();
    for _ in 0..80 {
        if posts("agentmux_answer") > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(posts("agentmux_answer"), 1, "the bot is woken with the answer");
    let body = seen.lock().unwrap().iter().find(|(_, b)| b.to_string().contains("agentmux_answer")).unwrap().1.to_string();
    assert!(body.contains("confirmed lines") && body.contains(&t) && body.contains("pass it on"), "{body}");

    // A bot still waiting in the same run gets the answer there: no second wake.
    let c2 = core.clone();
    let (b2, w2) = (bot.clone(), ws.clone());
    let asking = tokio::spawn(async move { tool(&c2, &b2, &w2, "mux_ask_peer", json!({"to": "claude-desktop", "question": "Which scene first?", "wait_s": 20})).await.unwrap().0 });
    let t2 = loop {
        if let Some(x) = v.open_tickets().unwrap().into_iter().find(|x| x["from"] == bot.as_str()) {
            break x["id"].as_str().unwrap().to_string();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    tool(&core, "claude-desktop", &root, "mux_reply", json!({"ticket": t2, "text": "Scene 1"})).await.unwrap();
    assert_eq!(asking.await.unwrap()["answer"], "Scene 1");
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(posts("agentmux_answer"), 1, "no duplicate wake for an answer the bot already has");

    // Agents without a webhook (Claude Code) aren't posted to.
    let t3 = core.start_ticket_in(&ctx("claude", &ws), None, None, "claude-desktop", "ok?", None).unwrap();
    core.finish(&t3, "answered", "yes", None).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(posts("agentmux_answer"), 1);
}
