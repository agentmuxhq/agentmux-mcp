//! Product rooms: folders are filed under a product, rooms can be renamed (everyone is told, and
//! bots that remember are woken) and merged, and a new folder prompts the agent to ask which
//! product it belongs to.

use agentmux_core::{mcp, providers::claude_code, webhook, CallerCtx, Core, Vault};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn ctx(agent: &str, cwd: &PathBuf) -> CallerCtx {
    CallerCtx { agent: agent.into(), cwd: cwd.to_string_lossy().into(), hop: 0 }
}

fn room_of(core: &Core, ticket: &str) -> String {
    core.vault.ticket(ticket).unwrap().unwrap()["roomId"].as_str().unwrap().to_string()
}

fn ask(core: &Arc<Core>, from: &str, cwd: &PathBuf) -> String {
    let t = core.start_ticket(&ctx(from, cwd), None, "grok", "q", None).unwrap();
    core.finish(&t, "answered", "ok", None).unwrap();
    room_of(core, &t)
}

/// A webhook endpoint that records request bodies.
async fn fake_webhook() -> (String, Arc<Mutex<Vec<String>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/automations/webhook/r1", l.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = l.accept().await else { return };
            let seen = s2.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut got = 0;
                loop {
                    let n = sock.read(&mut buf[got..]).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got += n;
                    let text = String::from_utf8_lossy(&buf[..got]).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let len: usize = text.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                        if got >= i + 4 + len {
                            seen.lock().unwrap().push(text[i + 4..].to_string());
                            break;
                        }
                    }
                }
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}").await;
            });
        }
    });
    (url, seen)
}

#[tokio::test]
async fn product_rooms() {
    // SAFETY: only test in this binary. The webhook key comes from the environment, never the keychain.
    unsafe { std::env::set_var("AGENTMUX_SECRET_GROK_WEBHOOK_KEY", "test-key") };
    // Outside any git repo: a folder inside one belongs to that repo's room.
    let dir = std::env::temp_dir().join(format!("agentmux-rooms-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (web, api, tools) = (dir.join("animation-web"), dir.join("animation-api"), dir.join("tools"));
    for d in [&web, &api, &tools] {
        std::fs::create_dir_all(d).unwrap();
    }
    let vault = Arc::new(Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap());
    let core = Core::with_vault(vault.clone());

    // A folder gets its own room the first time.
    let animation = ask(&core, "claude", &web);
    assert_eq!(core.vault.room(&animation).unwrap().unwrap()["title"], "animation-web");

    // Rename it to the product name: every thread is told, members get a notice.
    core.rename_room(&animation, "Animation", "user").unwrap();
    let r = core.vault.room(&animation).unwrap().unwrap();
    assert_eq!(r["title"], "Animation");
    let sess = core.vault.latest_session(&animation).unwrap().unwrap();
    let msgs = core.vault.messages(&sess).unwrap();
    assert!(msgs.iter().any(|m| m["type"] == "system" && m["text"].as_str().unwrap().contains("\"animation-web\" is now \"Animation\"")), "{msgs:?}");
    let ups = core.vault.updates_for("claude", false).unwrap();
    assert!(ups.iter().any(|u| u["kind"] == "notice" && u["question"].as_str().unwrap().contains("Animation")), "{ups:?}");
    let hook = claude_code::hook_context(&core.vault).unwrap().unwrap();
    assert!(hook.contains("Notice: Room renamed"), "{hook}");
    // Names stay unique.
    let other = ask(&core, "claude", &tools);
    assert!(core.rename_room(&other, "animation", "user").unwrap_err().to_string().contains("already exists"));

    // A second folder of the same product: the first ask hints at the existing products...
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": "mux_ask_peer", "arguments": {"to": "grok", "question": "hi", "wait_s": 0}}});
    let resp = mcp::handle(&core, &ctx("cursor", &api), call).await.unwrap();
    let view: Value = serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let note = view["room_note"].as_str().expect("new-folder hint");
    assert!(note.contains("Animation") && note.contains("mux_use_room"), "{note}");
    core.finish(view["ticket"].as_str().unwrap(), "answered", "ok", None).unwrap();
    // ...the agent files it under the product, and from then on it lands there.
    let (room, created) = core.use_room(&ctx("cursor", &api), "animation").unwrap();
    assert_eq!(room, animation);
    assert!(!created);
    assert_eq!(ask(&core, "cursor", &api), animation);
    assert_eq!(ask(&core, "claude", &web), animation, "the original folder still lands there");
    assert_eq!(core.vault.room(&animation).unwrap().unwrap()["folders"].as_array().unwrap().len(), 2);

    // A product that doesn't exist yet is created by name.
    let (billing, created) = core.use_room(&ctx("claude", &tools), "Billing").unwrap();
    assert!(created);
    assert_eq!(ask(&core, "claude", &tools), billing);

    // Merging moves threads, messages and folders, and removes the old room.
    let old = other; // the "tools" room, now empty of folders
    let before = core.vault.session_ids(&old).unwrap();
    core.merge_rooms(&old, &animation, "user").unwrap();
    assert!(core.vault.room(&old).unwrap().is_none());
    for s in before {
        assert_eq!(core.vault.session_room(&s).unwrap().as_deref(), Some(animation.as_str()));
    }
    assert!(core.merge_rooms(&animation, &animation, "user").is_err());

    // Rooms are found by name, and the vault reopens cleanly (migration is idempotent).
    assert_eq!(core.vault.find_room("ANIMATION").unwrap().as_deref(), Some(animation.as_str()));
    drop(core);
    let v2 = Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap();
    assert!(v2.room(&animation).unwrap().is_some());

    // A webhook bot in the room is woken with the rename (it keeps memory between runs).
    let core = Core::with_vault(Arc::new(v2));
    let (url, seen) = fake_webhook().await;
    webhook::save(&core.vault, "grok", &url, "test-key").unwrap();
    core.vault.add_member(&animation, "grok").unwrap();
    core.rename_room(&animation, "Animation Studio", "claude").unwrap();
    for _ in 0..50 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let bodies = seen.lock().unwrap().clone();
    assert!(bodies.iter().any(|b| b.contains("Animation Studio") && b.contains("notice")), "{bodies:?}");
    // The renamer isn't notified of their own rename.
    let ups = core.vault.updates_for("claude", true).unwrap();
    assert!(!ups.iter().any(|u| u["kind"] == "notice" && u["question"].as_str().unwrap().contains("Animation Studio")));
}
