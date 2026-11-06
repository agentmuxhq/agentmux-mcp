//! Deleting a room erases it for good (threads, messages, search, questions, attachments, the
//! agents' "current room"), tells the agents in it, leaves other rooms alone, and waits while an
//! agent is still answering there.

mod common;
use agentmux_core::{media, memory};
use common::*;
use serde_json::json;

const PNG_1PX: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[tokio::test]
async fn deleting_a_room_erases_it_and_tells_its_agents() {
    let (core, ws) = setup("delete-room");
    let v = &core.vault;

    // Junk room with a thread, a searchable message and an image.
    let junk_ws = ws.parent().unwrap().join("junk-room");
    std::fs::create_dir_all(&junk_ws).unwrap();
    let t = exchange(&core, "claude", &junk_ws, "grok", Some("Zebra plans"), "What about the zebra stripes?", "Striped.");
    let junk = v.ticket(&t).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();
    let sess = session_of(&core, &t);
    let (r, _) = tool(&core, "claude", &junk_ws, "mux_post", json!({"text": "zebra sketch", "session": sess,
        "attachments": [{"data": PNG_1PX, "name": "zebra.png"}]})).await.unwrap();
    let att = r["attachments"][0].as_str().unwrap().to_string();
    let file = media::get(v, &att).unwrap().unwrap()["path"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&file).exists());

    // A room to keep.
    let keep = exchange(&core, "claude", &ws, "cursor", Some("Keep me"), "Keep this zebra note too?", "Yes.");
    let keep_room = v.ticket(&keep).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();
    v.updates_for("grok", true).unwrap();

    // Not while an agent is still answering there.
    let running = core.start_ticket_in(&ctx("claude", &junk_ws), None, Some(&sess), "grok", "Still going?", None).unwrap();
    v.set_ticket_status(&running, "running").unwrap();
    assert!(core.delete_room(&junk, "user").unwrap_err().to_string().contains("still answering"));
    core.finish(&running, "answered", "done", None).unwrap();

    core.delete_room(&junk, "user").unwrap();
    assert!(v.room(&junk).unwrap().is_none());
    assert!(v.ticket(&t).unwrap().is_none(), "its questions are gone");
    assert!(v.messages(&sess).unwrap().is_empty(), "its messages are gone");
    assert!(!std::path::Path::new(&file).exists(), "its stored image is removed");
    // Search no longer finds it, but still finds the room that was kept.
    let hits = memory::search(v, "zebra", &memory::Scope::All, 10).await.unwrap();
    assert!(hits.iter().all(|h| h["session"] != sess.as_str()), "{hits:?}");
    assert!(hits.iter().any(|h| h["room"] == keep_room.as_str() || h["session"] == session_of(&core, &keep).as_str()), "{hits:?}");
    // Grok, who took part, is told.
    let ups = v.updates_for("grok", true).unwrap();
    assert!(ups.iter().any(|u| u["question"].as_str().unwrap_or("").contains("was deleted")), "{ups:?}");
    // The kept room is untouched; asking from the deleted room's folder starts a fresh room.
    assert!(v.room(&keep_room).unwrap().is_some());
    let again = exchange(&core, "claude", &junk_ws, "grok", Some("Fresh"), "New start?", "Yes.");
    let fresh = v.ticket(&again).unwrap().unwrap()["roomId"].as_str().unwrap().to_string();
    assert_eq!(v.session_ids(&fresh).unwrap(), vec![session_of(&core, &again)], "nothing old came back");
    assert!(core.delete_room("room-nope", "user").is_err());
}
