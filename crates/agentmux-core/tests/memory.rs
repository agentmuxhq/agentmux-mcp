//! Memory: search across products (words, thread names, summaries, meaning), contextual summaries
//! (outline without an engine, written with one, outline again if the writer misbehaves), product
//! briefs, background upkeep, and mux_brief handing over or answering from memory.

mod common;
use agentmux_core::memory::{self, Scope};
use common::*;
use serde_json::{json, Value};
use std::sync::Arc;

/// A toy embedding: one dimension per concept, so meaning can match without shared words.
fn embed_text(t: &str) -> Vec<f64> {
    let t = t.to_lowercase();
    let has = |ws: &[&str]| if ws.iter().any(|w| t.contains(w)) { 1.0 } else { 0.0 };
    vec![
        has(&["animation", "sprite", "handoff", "frames", "keyframe"]),
        has(&["invoice", "billing", "payment", "stripe"]),
        has(&["release", "launch", "ship"]),
        0.05,
    ]
}

#[tokio::test]
async fn memory_search_summaries_and_briefs() {
    // SAFETY: only test in this binary that sets these; keychain stays untouched.
    unsafe { std::env::set_var("AGENTMUX_SECRET_ENGINE_API_KEY", "test-key") };
    let (core, ws) = setup("memory");
    let v = &core.vault;

    // Two products, each with a thread.
    let t1 = exchange(&core, "claude", &ws, "grok", Some("Animation handoff"),
        "Grok, hand the animation to Claude: which keyframe timings did we settle on? See https://example.com/spec and src/anim/timeline.ts",
        "We decided to use 24 fps with eased keyframes. Claude should take over the timeline in src/anim/timeline.ts.");
    let handoff = session_of(&core, &t1);
    let room = v.session_room(&handoff).unwrap().unwrap();
    v.rename_room(&room, "Animation").unwrap();
    let billing_ws = ws.parent().unwrap().join("billing");
    std::fs::create_dir_all(&billing_ws).unwrap();
    let t2 = exchange(&core, "cursor", &billing_ws, "claude", Some("Invoices"), "How do we retry failed invoice payments?", "Retry three times with Stripe's smart retries.");
    let invoices = session_of(&core, &t2);
    // An open question in the handoff thread.
    let open = core.start_ticket_in(&ctx("claude", &ws), None, Some(&handoff), "cursor", "Can you check the timeline tests?", None).unwrap();

    // Words and thread names, across products.
    let hits = memory::search(v, "what happened to the animation handoff?", &Scope::All, 5).await.unwrap();
    assert_eq!(hits[0]["session"], handoff.as_str(), "{hits:#?}");
    assert_eq!(hits[0]["roomName"], "Animation");
    assert!(hits[0]["matched"].as_array().unwrap().iter().any(|m| m == "thread name"));
    let hits = memory::search(v, "invoice retries", &Scope::All, 5).await.unwrap();
    assert_eq!(hits[0]["session"], invoices.as_str());
    assert!(memory::search(v, "the", &Scope::All, 5).await.unwrap().is_empty(), "filler words alone find nothing");

    // A cloud bot only sees threads it took part in.
    let scoped = memory::search(v, "invoice", &Scope::Participant("grok".into()), 5).await.unwrap();
    assert!(scoped.iter().all(|h| h["session"] != invoices.as_str()), "{scoped:#?}");
    assert!(!memory::search(v, "animation", &Scope::Participant("grok".into()), 5).await.unwrap().is_empty());

    // Without a writer: a structured outline with every section.
    let (sum, engine) = memory::summarize(v, &handoff).await.unwrap();
    assert_eq!(engine, "outline");
    for h in memory::SUMMARY_HEADINGS {
        assert!(sum.contains(&format!("## {h}")), "missing {h}: {sum}");
    }
    assert!(sum.contains("keyframe timings"), "goal is the first question: {sum}");
    assert!(sum.contains("24 fps"), "decision captured: {sum}");
    assert!(sum.contains("check the timeline tests"), "open item listed: {sum}");
    assert!(sum.contains("https://example.com/spec") && sum.contains("src/anim/timeline.ts"), "artifacts: {sum}");
    let _ = open;

    // With a writer (a fake Ollama): the written summary, and the prompt carried the thread.
    let (url, seen) = fake_server(Arc::new(|path: &str, body: &Value| {
        match path {
            "/api/chat" => {
                let prompt = body["messages"][1]["content"].as_str().unwrap_or("");
                if prompt.contains("Question:") {
                    (200, json!({"message": {"content": "They settled on 24 fps with eased keyframes."}}))
                } else if body["model"] == "garbage" {
                    (200, json!({"message": {"content": "sure! here's a summary"}}))
                } else {
                    (200, json!({"message": {"content": "## Goal\n- Hand the animation to Claude\n## Decisions\n- 24 fps, because the designs use it\n## Current state\n- Claude owns the timeline\n## Open items\n- Timeline tests\n## Who's involved\n- Claude, Grok\n## Key artifacts\n- src/anim/timeline.ts"}}))
                }
            }
            "/api/embed" => {
                let input = body["input"].as_array().cloned().unwrap_or_default();
                (200, json!({"embeddings": input.iter().map(|t| embed_text(t.as_str().unwrap_or(""))).collect::<Vec<_>>()}))
            }
            _ => (404, json!({"error": "no"})),
        }
    }))
    .await;
    agentmux_core::engines::configure(v, "writer", &json!({"kind": "ollama", "url": url, "model": "qwen2.5:3b"})).unwrap();
    v.add_message(&handoff, &room, "grok", Some("claude"), "a2a_reply", "Also: Claude owns the timeline now.", None, None).unwrap();
    let (sum, engine) = memory::summarize(v, &handoff).await.unwrap();
    assert_eq!(engine, "ollama:qwen2.5:3b");
    assert!(sum.contains("Claude owns the timeline"), "{sum}");
    let prompt = seen.lock().unwrap().iter().rev().find(|(p, _)| p == "/api/chat").unwrap().1["messages"][1]["content"].as_str().unwrap().to_string();
    assert!(prompt.contains("Product: Animation") && prompt.contains("Thread: Animation handoff") && prompt.contains("Current summary"), "{prompt}");

    // A writer that doesn't follow the format: fall back to the outline, never store junk.
    agentmux_core::engines::configure(v, "writer", &json!({"model": "garbage"})).unwrap();
    v.add_message(&handoff, &room, "claude", Some("grok"), "a2a_request", "One more thing.", None, None).unwrap();
    let (_, engine) = memory::summarize(v, &handoff).await.unwrap();
    assert_eq!(engine, "outline");
    agentmux_core::engines::configure(v, "writer", &json!({"model": "qwen2.5:3b"})).unwrap();

    // Meaning: with embeddings, "sprite frames" finds the handoff thread with no shared word.
    agentmux_core::engines::configure(v, "embed", &json!({"kind": "ollama", "url": url, "model": "nomic-embed-text"})).unwrap();
    memory::tick(v, 0, 10).await.unwrap();
    let n: i64 = v.messages(&handoff).unwrap().len() as i64;
    assert!(n > 0);
    let hits = memory::search(v, "sprite frames", &Scope::All, 3).await.unwrap();
    assert_eq!(hits.first().map(|h| h["session"].clone()), Some(json!(handoff)), "{hits:#?}");
    assert!(hits[0]["matched"].as_array().unwrap().iter().any(|m| m == "meaning"));

    // Product brief lists its threads with where they stand.
    let b = memory::brief(v, &room).await.unwrap();
    assert!(b.starts_with("# Animation") && b.contains("Animation handoff"), "{b}");

    // mux_brief: find by question, hand over the summary and recent messages, and answer.
    let (r, _) = tool(&core, "cursor", &billing_ws, "mux_brief", json!({"question": "what happened to the animation handoff?"})).await.unwrap();
    assert_eq!(r["session"], handoff.as_str());
    assert!(r["summary"].as_str().unwrap().contains("## Current state"));
    assert!(!r["recent"].as_array().unwrap().is_empty());
    assert_eq!(r["answer"], "They settled on 24 fps with eased keyframes.");
    // ...and no thread asked for: this product's brief.
    let (r, _) = tool(&core, "cursor", &billing_ws, "mux_brief", json!({})).await.unwrap();
    assert!(r["brief"].as_str().unwrap().contains("Invoices"), "{r}");

    // mux_search as a tool.
    let (r, _) = tool(&core, "claude", &ws, "mux_search", json!({"query": "stripe retries"})).await.unwrap();
    assert_eq!(r["results"][0]["session"], invoices.as_str());
}
