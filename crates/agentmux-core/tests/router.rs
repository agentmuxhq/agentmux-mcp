//! Routing a topic to a thread: exact names, sure matches, "unsure → ask the user" with the answer
//! logged, decision engines (Jeff, local LLM) with confidence thresholds, cloud bots never asked,
//! and the stats that say whether a stronger decision model is worth it.

mod common;
use agentmux_core::{engines, router, router::Route};
use common::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn routing_topics_to_threads() {
    unsafe { std::env::set_var("AGENTMUX_SECRET_ENGINE_API_KEY", "test-key") };
    let (core, ws) = setup("router");
    let v = &core.vault;
    let t = exchange(&core, "claude", &ws, "grok", Some("Release plan"), "When do we ship 0.4?", "Next Tuesday after QA.");
    let release = session_of(&core, &t);
    let room = v.session_room(&release).unwrap().unwrap();
    let qa = session_of(&core, &exchange(&core, "claude", &ws, "grok", Some("QA checklist"), "What's on the QA list?", "Smoke tests and the tunnel."));

    // Exact topic: no doubt.
    match router::route(v, "claude", &room, "the release plan", "x", true).await.unwrap() {
        Route::Use { session, method, .. } => assert_eq!((session.as_str(), method), (release.as_str(), "topic")),
        r => panic!("{r:?}"),
    }
    // Nothing similar: a new thread.
    assert!(matches!(router::route(v, "claude", &room, "zebra migration", "x", true).await.unwrap(), Route::New));
    // Similar but not the same name: ask the user, with the candidates.
    let options = match router::route(v, "claude", &room, "release schedule", "when do we ship?", true).await.unwrap() {
        Route::Ask { options } => options,
        r => panic!("expected ask, got {r:?}"),
    };
    assert_eq!(options[0]["session"], release.as_str(), "{options:#?}");
    // A cloud bot can't be asked: best guess (here, the release plan) instead.
    match router::route(v, "grok", &room, "release schedule", "x", false).await.unwrap() {
        Route::Use { session, method, .. } => assert_eq!((session.as_str(), method), (release.as_str(), "best-guess")),
        Route::New => {}
        r => panic!("{r:?}"),
    }

    // Through the tool: needs_choice, then the user's pick routes the question and is logged.
    let (r, _) = tool(&core, "claude", &ws, "mux_ask_peer", json!({"to": "grok", "question": "Is Tuesday still on?", "topic": "release schedule", "wait_s": 0})).await.unwrap();
    assert_eq!(r["status"], "needs_choice", "{r}");
    assert!(r["options"].as_array().unwrap().iter().any(|o| o["thread"] == "Release plan"));
    let (r, _) = tool(&core, "claude", &ws, "mux_ask_peer", json!({"to": "grok", "question": "Is Tuesday still on?", "topic": "release schedule", "session": release, "wait_s": 0})).await.unwrap();
    assert_eq!(session_of(&core, r["ticket"].as_str().unwrap()), release);
    assert_eq!(r["thread"], "Release plan");
    core.finish(r["ticket"].as_str().unwrap(), "answered", "yes", None).unwrap();
    let st = router::stats(v).unwrap();
    assert!(st["answered"].as_i64().unwrap() >= 1 && st["agreedWithRouter"].as_i64().unwrap() >= 1, "{st}");
    assert!(st["advice"].as_str().unwrap().contains("Not enough"), "{st}");

    // Jeff as the decision engine: confident → routed; "new" → new thread; unsure → ask.
    let reply: Arc<Mutex<Value>> = Arc::new(Mutex::new(json!({})));
    let r2 = reply.clone();
    let (url, seen) = fake_server(Arc::new(move |path: &str, _b: &Value| {
        if path == "/v1/systemone" { (200, r2.lock().unwrap().clone()) } else { (404, json!({})) }
    }))
    .await;
    engines::configure(v, "decide", &json!({"kind": "jeff", "url": url})).unwrap();
    *reply.lock().unwrap() = json!({"answers": {"pick": {"type": "choice", "choice": "t1", "probabilities": {"t1": 0.93, "new": 0.07}, "confidence": 0.9}}});
    match router::route(v, "claude", &room, "ship date", "when do we ship?", true).await.unwrap() {
        Route::Use { method, confidence, .. } => assert_eq!((method, confidence > 0.8), ("decided", true)),
        r => panic!("{r:?}"),
    }
    let req = seen.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(req["model"], "ground", "Jeff's ranking adapter by default");
    assert!(req["questions"]["pick"]["criteria"]["new"].is_string() && req["state"].as_str().unwrap().contains("ship date"), "{req}");
    *reply.lock().unwrap() = json!({"answers": {"pick": {"type": "choice", "choice": "new", "probabilities": {"t1": 0.1, "new": 0.9}, "confidence": 0.85}}});
    assert!(matches!(router::route(v, "claude", &room, "release mascot", "x", true).await.unwrap(), Route::New));
    *reply.lock().unwrap() = json!({"answers": {"pick": {"type": "choice", "choice": "t1", "probabilities": {"t1": 0.5, "new": 0.5}, "confidence": 0.2}}});
    assert!(matches!(router::route(v, "claude", &room, "release schedule", "x", true).await.unwrap(), Route::Ask { .. }));
    // A broken decision engine falls back to the built-in rules instead of failing the question.
    *reply.lock().unwrap() = json!({"oops": true});
    assert!(router::route(v, "claude", &room, "release schedule", "x", true).await.is_ok());

    // The local LLM as the decision engine (answers JSON through the writer).
    let (wurl, _) = fake_server(Arc::new(|path: &str, _b: &Value| {
        if path == "/api/chat" { (200, json!({"message": {"content": "{\"choice\": \"t1\", \"confidence\": 0.92}"}})) } else { (404, json!({})) }
    }))
    .await;
    engines::configure(v, "writer", &json!({"kind": "ollama", "url": wurl, "model": "qwen2.5:3b"})).unwrap();
    engines::configure(v, "decide", &json!({"kind": "llm"})).unwrap();
    match router::route(v, "claude", &room, "QA plan", "what's left to test?", true).await.unwrap() {
        Route::Use { method, .. } => assert_eq!(method, "decided"),
        r => panic!("{r:?}"),
    }
    let _ = qa;
    let st = router::stats(v).unwrap();
    assert!(st["byDecisionEngine"].as_i64().unwrap() >= 3, "{st}");
    assert!(st["byMethod"]["asked"].as_i64().unwrap() >= 2);
}
