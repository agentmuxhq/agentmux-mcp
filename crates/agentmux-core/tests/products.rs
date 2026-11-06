//! Agents that start AgentMux outside a project (Cursor in your home folder, Codex in a scratch
//! folder) get their questions filed by what they're about, never in a room named after the folder.

mod common;
use agentmux_core::router::{self, mentioned_products, ProductRoute};
use common::*;
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn questions_from_home_go_to_their_product() {
    let (core, ws) = setup("products");
    let home = ws.parent().unwrap().join("home-diablo");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: only test in this binary. Cursor runs its MCP server in the user's home folder.
    unsafe { std::env::set_var("HOME", &home) };
    let v = &core.vault;

    // Two real products, each with history.
    let sqlite_ws = ws.parent().unwrap().join("sqlitebuilder-web");
    std::fs::create_dir_all(&sqlite_ws).unwrap();
    exchange(&core, "claude", &sqlite_ws, "grok", Some("Guest uploads"), "How do guest .db uploads work on sqlitebuilder.com?", "Read-only guest sessions.");
    let sqlite_room = v.find_room("sqlitebuilder-web").unwrap().unwrap();
    v.rename_room(&sqlite_room, "SQLite Builder").unwrap();
    let apito = v.create_room("Apito").unwrap();

    // Cursor, from the home folder, asks about the SQLite Builder guest explorer: filed there.
    let (r, _) = tool(&core, "cursor", &home, "mux_ask_peer", json!({"to": "grok", "topic": "guest explorer iframe", "wait_s": 0,
        "question": "Guest SQLite browse redirects to https://app.sqlitebuilder.com/guest/explore. Should the explorer stay inside the marketing page in an iframe?"})).await.unwrap();
    let t = r["ticket"].as_str().unwrap_or_else(|| panic!("asked, not unsure: {r}")).to_string();
    assert_eq!(v.ticket(&t).unwrap().unwrap()["roomId"], sqlite_room.as_str(), "{r}");
    assert_eq!(r["thread"], "guest explorer iframe", "a new thread named by the topic: {r}");
    core.finish(&t, "answered", "Keep it on the marketing origin.", None).unwrap();

    // The Apito question goes to Apito.
    let (r, _) = tool(&core, "cursor", &home, "mux_ask_peer", json!({"to": "grok", "topic": "mock api keep this", "wait_s": 0,
        "question": "On apito.io/tools/mock-api, 'Keep this' jumps to app.apito.io. Should it stay on the marketing page until claim?"})).await.unwrap();
    let t = r["ticket"].as_str().expect("filed").to_string();
    assert_eq!(v.ticket(&t).unwrap().unwrap()["roomId"], apito.as_str(), "{r}");
    core.finish(&t, "answered", "Stay until claim.", None).unwrap();

    // Nothing points anywhere specific: AgentMux asks, offering the products and a new name.
    let (r, _) = tool(&core, "codex-like", &home, "mux_ask_peer", json!({"to": "grok", "topic": "payments", "question": "Should we retry declined cards?", "wait_s": 0})).await
        .unwrap_or_else(|_| (json!({}), json!({})));
    let _ = r;
    let (r, _) = tool(&core, "cursor", &home, "mux_ask_peer", json!({"to": "claude", "topic": "explorer", "question": "What about the explorer guest flow?", "wait_s": 0})).await.unwrap();
    if r["status"] == "needs_choice" {
        assert_eq!(r["choice"], "product");
        assert!(r["products"].as_array().unwrap().iter().any(|p| p["room"] == "SQLite Builder"), "{r}");
        assert!(r["suggestedNewProduct"].is_string());
    } else {
        assert_eq!(v.ticket(r["ticket"].as_str().unwrap()).unwrap().unwrap()["roomId"], sqlite_room.as_str(), "routed confidently instead: {r}");
        core.finish(r["ticket"].as_str().unwrap(), "answered", "ok", None).unwrap();
    }
    // The user answers with a new product name: it's created and used.
    let (r, _) = tool(&core, "cursor", &home, "mux_ask_peer", json!({"to": "grok", "room": "Payments Revamp", "question": "Should we retry declined cards?", "wait_s": 0})).await.unwrap();
    let pay = v.find_room("Payments Revamp").unwrap().expect("room created from the user's answer");
    assert_eq!(v.ticket(r["ticket"].as_str().unwrap()).unwrap().unwrap()["roomId"], pay.as_str());
    core.finish(r["ticket"].as_str().unwrap(), "answered", "Yes, twice.", None).unwrap();

    // A product nobody has mentioned before: a new room named after it.
    let (r, _) = tool(&core, "cursor", &home, "mux_ask_peer", json!({"to": "grok", "topic": "onboarding", "wait_s": 0,
        "question": "For brandnewtool.dev, what should the onboarding checklist be?"})).await.unwrap();
    let room = v.ticket(r["ticket"].as_str().unwrap_or_else(|| panic!("{r}"))).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();
    assert_eq!(v.room(&room).unwrap().unwrap()["title"], "Brandnewtool");

    // Never a room named after the home folder; tools without a room use the current product.
    let names: Vec<String> = v.room_ids().unwrap().iter().filter_map(|id| v.room(id).unwrap().map(|r| r["title"].as_str().unwrap().to_string())).collect();
    assert!(!names.iter().any(|n| n == "home-diablo"), "{names:?}");
    let (r, _) = tool(&core, "cursor", &home, "mux_sessions", json!({})).await.unwrap();
    assert_eq!(r["roomName"], "Brandnewtool");

    // Website names are read from text; the writer engine names new products when it's on.
    assert_eq!(mentioned_products("see https://app.apito.io/x and sqlitebuilder.com, not localhost:4000"), vec!["apito", "sqlitebuilder"]);
    unsafe { std::env::set_var("AGENTMUX_SECRET_ENGINE_API_KEY", "k") };
    let (url, _) = fake_server(Arc::new(|path: &str, _b: &Value| {
        if path == "/api/chat" { (200, json!({"message": {"content": "\"Apito Website Refactor\""}})) } else { (404, json!({})) }
    }))
    .await;
    agentmux_core::engines::configure(v, "writer", &json!({"kind": "ollama", "url": url, "model": "m"})).unwrap();
    assert_eq!(router::suggest_room_name(v, Some("site"), "refactor the marketing site").await, "Apito Website Refactor");
    match router::route_product(v, "cursor", Some("zebra"), "unrelated zebra question", true).await.unwrap() {
        ProductRoute::New { name } => assert_eq!(name, "Apito Website Refactor"),
        other => panic!("{other:?}"),
    }
}
