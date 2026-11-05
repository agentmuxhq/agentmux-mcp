//! Group threads (every member hears about every message; ask everyone at once), media
//! (attach, limits, captions searchable, images returned as images, cloud-bot rules), and Claude
//! Code chats bound to their AgentMux thread through the hook.

mod common;
use agentmux_core::{media, memory, providers::claude_code};
use base64::Engine as _;
use common::*;
use serde_json::json;

const PNG_1PX: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[tokio::test]
async fn group_threads_media_and_claude_chats() {
    unsafe { std::env::set_var("AGENTMUX_WAKE_TIMEOUT_SECS", "2") };
    let (core, ws) = setup("collab");
    let v = &core.vault;

    // ── Group thread: claude, grok and cursor all took part in "Handoff".
    let t = exchange(&core, "claude", &ws, "grok", Some("Handoff"), "Start the handoff", "ok");
    let handoff = session_of(&core, &t);
    let t2 = core.start_ticket_in(&ctx("cursor", &ws), None, Some(&handoff), "claude", "I'll review the code", None).unwrap();
    core.finish(&t2, "answered", "thanks", None).unwrap();
    assert_eq!(v.session_members(&handoff).unwrap(), vec!["claude", "cursor", "grok"]);
    // A new exchange between two of them: the third hears about it.
    v.updates_for("cursor", true).unwrap();
    let t3 = core.start_ticket_in(&ctx("claude", &ws), None, Some(&handoff), "grok", "Frames exported?", None).unwrap();
    core.finish(&t3, "answered", "Yes, 120 frames", None).unwrap();
    let ups = v.updates_for("cursor", true).unwrap();
    let notes: Vec<&str> = ups.iter().filter(|u| u["kind"] == "notice").map(|u| u["question"].as_str().unwrap()).collect();
    assert!(notes.iter().any(|n| n.contains("Handoff") && n.contains("Frames exported?")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("Yes, 120 frames")), "{notes:?}");
    // The two in the exchange aren't notified of their own messages.
    assert!(!v.updates_for("grok", true).unwrap().iter().any(|u| u["kind"] == "notice" && u["question"].as_str().unwrap().contains("Frames exported")));

    // Ask everyone at once: grok (inbox) and cursor (no CLI here, so its wake fails fast).
    let (r, _) = tool(&core, "claude", &ws, "mux_ask_all", json!({"question": "Ready to ship?", "session": handoff, "wait_s": 4})).await.unwrap();
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "{r}");
    let to: Vec<&str> = results.iter().map(|x| x["to"].as_str().unwrap()).collect();
    assert!(to.contains(&"grok") && to.contains(&"cursor"));
    assert_eq!(r["thread"], "Handoff");
    // Every question landed in the same thread.
    for x in results {
        if let Some(tk) = x["ticket"].as_str() {
            assert_eq!(session_of(&core, tk), handoff);
            let _ = core.finish(tk, "answered", "ready", None);
        }
    }
    let solo = exchange(&core, "claude", &ws.parent().unwrap().to_path_buf(), "grok", Some("Solo"), "x", "y");
    let solo_sess = session_of(&core, &solo);
    let (r, _) = tool(&core, "cursor", &ws, "mux_ask_all", json!({"question": "anyone?", "session": solo_sess, "wait_s": 0})).await.unwrap();
    for x in r["results"].as_array().unwrap() {
        if let Some(tk) = x["ticket"].as_str() {
            let _ = core.finish(tk, "answered", "here", None);
        }
    }

    // ── Media.
    let img = ws.join("frame.png");
    std::fs::write(&img, base64::engine::general_purpose::STANDARD.decode(PNG_1PX).unwrap()).unwrap();
    let (r, _) = tool(&core, "claude", &ws, "mux_ask_peer", json!({"to": "grok", "question": "Does this frame look right?", "session": handoff,
        "attachments": [{"path": img.to_string_lossy(), "caption": "walk cycle keyframe 12"}], "wait_s": 0})).await.unwrap();
    let tk = r["ticket"].as_str().unwrap().to_string();
    let req_atts = media::for_ticket_request(v, &tk).unwrap();
    assert_eq!(req_atts.len(), 1);
    assert_eq!(req_atts[0]["mime"], "image/png");
    let stored = req_atts[0]["path"].as_str().unwrap();
    assert!(stored.contains("/media/") && std::path::Path::new(stored).exists(), "copied into AgentMux's media folder");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(stored).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // Caption is searchable.
    let hits = memory::search(v, "walk cycle keyframe", &memory::Scope::All, 3).await.unwrap();
    assert_eq!(hits[0]["session"], handoff.as_str());
    // Grok answers with an image (as base64, the way a cloud bot sends files)…
    let (_, _) = tool(&core, "grok", &ws, "mux_reply", json!({"ticket": tk, "text": "Here's the fixed frame",
        "attachments": [{"data": PNG_1PX, "name": "fixed.png", "caption": "keyframe 12 fixed"}]})).await.unwrap();
    // …and Claude gets it back as an actual image block.
    let (view, raw) = tool(&core, "claude", &ws, "mux_get_reply", json!({"ticket": tk})).await.unwrap();
    assert_eq!(view["attachments"][0]["name"], "fixed.png");
    let blocks = raw["content"].as_array().unwrap();
    assert!(blocks.iter().any(|b| b["type"] == "image" && b["mimeType"] == "image/png"), "{blocks:?}");
    // Cloud bots can't attach by path, and can only fetch attachments from their own threads.
    let err = tool(&core, "grok", &ws, "mux_post", json!({"text": "x", "attachments": [{"path": img.to_string_lossy()}]})).await.unwrap_err();
    assert!(err.contains("base64"), "{err}");
    let att_id = req_atts[0]["id"].as_str().unwrap();
    let (got, _) = tool(&core, "grok", &ws, "mux_attachment", json!({"id": att_id})).await.unwrap();
    assert!(got["data"].as_str().unwrap().len() > 20);
    let other = exchange(&core, "claude", &ws, "cursor", Some("Private"), "secret plan", "ok");
    let (r, _) = tool(&core, "claude", &ws, "mux_post", json!({"text": "plan attached", "session": session_of(&core, &other),
        "attachments": [{"data": PNG_1PX, "name": "plan.png"}]})).await.unwrap();
    let plan_id = r["attachments"][0].as_str().unwrap().to_string();
    if media::session_of(v, &plan_id).unwrap().as_deref() != Some(handoff.as_str()) {
        assert!(tool(&core, "grok:other", &ws, "mux_attachment", json!({"id": plan_id})).await.is_err());
    }
    // Limits: unknown types and oversized files are refused.
    let exe = ws.join("tool.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    assert!(tool(&core, "claude", &ws, "mux_post", json!({"text": "x", "attachments": [{"path": exe.to_string_lossy()}]})).await.unwrap_err().contains("only images"));
    v.set_setting("media.max_mb", "0").unwrap();
    assert!(tool(&core, "claude", &ws, "mux_post", json!({"text": "x", "attachments": [{"path": img.to_string_lossy()}]})).await.unwrap_err().contains("limit"));
    v.set_setting("media.max_mb", "50").unwrap();

    // ── Claude Code chats bound to threads through the hook.
    let chat_a = claude_code::HookEvent { event: "UserPromptSubmit".into(), chat: Some("chat-a".into()), cwd: Some(ws.to_string_lossy().into()) };
    claude_code::on_hook(v, &chat_a).unwrap();
    let ta = exchange(&core, "claude", &ws, "cursor", Some("Animation export"), "Export the frames?", "On it.");
    let bound = session_of(&core, &ta);
    // Talking to someone else from the same chat, without a topic, stays in that chat's thread.
    let tb = exchange(&core, "claude", &ws, "grok", None, "Anything to add?", "No.");
    assert_eq!(session_of(&core, &tb), bound);
    // Another chat in the same folder gets its own thread.
    let chat_b = claude_code::HookEvent { event: "UserPromptSubmit".into(), chat: Some("chat-b".into()), cwd: Some(ws.to_string_lossy().into()) };
    claude_code::on_hook(v, &chat_b).unwrap();
    let tc = exchange(&core, "claude", &ws, "grok", Some("Billing"), "Invoice status?", "Paid.");
    assert_ne!(session_of(&core, &tc), bound);
    // Coming back to chat A: the hook says which thread it's on, and questions continue there.
    let start_a = claude_code::HookEvent { event: "SessionStart".into(), ..chat_a.clone() };
    let lead = claude_code::on_hook(v, &start_a).unwrap().unwrap();
    assert!(lead.contains("Animation export"), "{lead}");
    let td = exchange(&core, "claude", &ws, "cursor", None, "Done?", "Yes.");
    assert_eq!(session_of(&core, &td), bound);

    // Gemini CLI chats work the same way through its own hook events.
    use agentmux_core::providers::{chat_hook, gemini};
    let g = chat_hook::HookEvent { event: "BeforeAgent".into(), chat: Some("gem-1".into()), cwd: Some(ws.to_string_lossy().into()) };
    chat_hook::on_hook(&gemini::HOOK, v, &g).unwrap();
    let tg = exchange(&core, "gemini", &ws, "claude", Some("Gemini review"), "Review the export?", "Looks good.");
    let gem_thread = session_of(&core, &tg);
    assert_eq!(session_of(&core, &exchange(&core, "gemini", &ws, "cursor", None, "And you?", "Fine.")), gem_thread);
    // Claude answered Gemini: on Gemini's next message the hook carries it (Gemini wants JSON; the CLI wraps it).
    let ask = core.start_ticket_in(&ctx("claude", &ws), None, Some(&gem_thread), "gemini", "Can you double-check frame 12?", None).unwrap();
    let text = chat_hook::on_hook(&gemini::HOOK, v, &g).unwrap().unwrap();
    assert!(text.contains("frame 12") && text.contains(&ask), "{text}");
    let start = chat_hook::HookEvent { event: "SessionStart".into(), ..g.clone() };
    assert!(chat_hook::on_hook(&gemini::HOOK, v, &start).unwrap().unwrap().contains("Gemini review"));
}
