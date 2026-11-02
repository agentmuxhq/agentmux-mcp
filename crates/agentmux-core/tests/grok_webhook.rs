//! Grok wake path against a fake local webhook server (no network, no keychain).

use agentmux_core::{mcp, webhook, CallerCtx, Core, Vault};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: &str = "test-key-do-not-leak";
static ENV: Once = Once::new();

fn init_env() {
    ENV.call_once(|| {
        // Safe: runs once before any test reads these.
        unsafe {
            std::env::set_var("AGENTMUX_SECRET_GROK_WEBHOOK_KEY", KEY);
            std::env::set_var("AGENTMUX_SECRET_GROK_WEBHOOK_KEY_JOB_HUNTER", "job-hunter-key");
            std::env::set_var("AGENTMUX_WAKE_TIMEOUT_SECS", "2");
        }
    });
}

#[derive(Clone, Debug, Default)]
struct Captured {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Captured {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

/// Minimal one-shot-per-connection HTTP server that records requests.
async fn fake_grok(status: u16, body: &str) -> (String, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let (seen2, body) = (seen.clone(), body.to_string());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let (seen, body) = (seen2.clone(), body.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.lines();
                let mut cap = Captured { request_line: lines.next().unwrap_or("").to_string(), ..Default::default() };
                for l in lines {
                    if let Some((k, v)) = l.split_once(':') {
                        cap.headers.push((k.trim().to_lowercase(), v.trim().to_string()));
                    }
                }
                let want: usize = cap.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                while buf.len() < head_end + 4 + want {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                cap.body = String::from_utf8_lossy(&buf[head_end + 4..]).to_string();
                seen.lock().unwrap().push(cap);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (format!("http://127.0.0.1:{}/automations/webhook/routine-abc123", addr.port()), seen)
}

fn test_core(name: &str) -> (Arc<Core>, String) {
    init_env();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("grok-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let vault = Arc::new(Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap());
    (Core::with_vault(vault), dir.join("ws").to_string_lossy().to_string())
}

fn configure(core: &Core, url: &str) {
    core.vault.set_setting("grok.webhook_url", url).unwrap();
    core.vault.set_wake_kind("grok", "webhook").unwrap();
}

fn ctx(agent: &str, cwd: &str) -> CallerCtx {
    CallerCtx { agent: agent.into(), cwd: cwd.into(), hop: 0 }
}

async fn tool(core: &Arc<Core>, who: &CallerCtx, name: &str, args: Value) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let resp = mcp::handle(core, who, req).await.unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("null");
    serde_json::from_str(text).unwrap_or(json!(text))
}

#[tokio::test]
async fn webhook_wake_round_trip() {
    let (url, seen) = fake_grok(200, r#"{"runUuid":"run-1"}"#).await;
    let (core, ws) = test_core("roundtrip");
    configure(&core, &url);

    let cursor = ctx("cursor", &ws);
    let ticket = core.start_ticket(&cursor, None, "grok", "whats up with the job hunter bot?", None).unwrap();
    // Webhook is fired in the background; it only starts a run and must not close the ticket.
    let t = core.wait_ticket(&ticket, 1).await.unwrap();
    assert_eq!(t["status"], "running", "webhook accepted => ticket stays open until mux_reply");

    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert!(r.request_line.starts_with("POST /automations/webhook/routine-abc123 "), "{}", r.request_line);
    assert_eq!(r.header("authorization"), Some(format!("Bearer {KEY}").as_str()));
    assert!(r.header("content-type").unwrap().starts_with("application/json"));
    let body: Value = serde_json::from_str(&r.body).unwrap();
    let c = body["context"].as_str().unwrap();
    assert!(c.starts_with(&format!("agentmux_ticket={ticket}")));
    assert!(c.contains("whats up with the job hunter bot?"));
    assert!(c.contains("mux_reply"));
    assert!(!c.contains(KEY), "key must never be put in the context");

    // Grok's routine picks the ticket up and answers over MCP.
    let grok = ctx("grok", &ws);
    let inbox = tool(&core, &grok, "mux_inbox", json!({})).await;
    assert_eq!(inbox[0]["ticket"], ticket.as_str());
    let ok = tool(&core, &grok, "mux_reply", json!({"ticket": ticket, "text": "14 new listings, 2 auth errors"})).await;
    assert_eq!(ok["accepted"], true);

    let done = tool(&core, &cursor, "mux_get_reply", json!({"ticket": ticket, "wait_s": 1})).await;
    assert_eq!(done["status"], "answered");
    assert_eq!(done["from"], "grok");
    assert_eq!(done["answer"], "14 new listings, 2 auth errors");
}

#[tokio::test]
async fn webhook_http_error_fails_ticket_without_leaking_key() {
    let (url, seen) = fake_grok(404, r#"{"error":"routine not found"}"#).await;
    let (core, ws) = test_core("http404");
    configure(&core, &url);

    let ticket = core.start_ticket(&ctx("cursor", &ws), None, "grok", "ping", None).unwrap();
    let t = core.wait_ticket(&ticket, 5).await.unwrap();
    assert_eq!(t["status"], "failed");
    let msg = t["answer"].as_str().unwrap();
    assert!(msg.contains("HTTP 404") && msg.contains("routine not found"), "{msg}");
    assert!(!msg.contains(KEY));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn webhook_without_reply_times_out_with_actionable_error() {
    let (url, _seen) = fake_grok(200, r#"{"runUuid":"run-9"}"#).await;
    let (core, ws) = test_core("timeout");
    configure(&core, &url);

    let ticket = core.start_ticket(&ctx("cursor", &ws), None, "grok", "anyone there?", None).unwrap();
    let t = core.wait_ticket(&ticket, 8).await.unwrap(); // wake timeout is 2s in tests
    assert_eq!(t["status"], "expired");
    let msg = t["answer"].as_str().unwrap();
    assert!(msg.contains("run-9") && msg.contains("mux_reply"), "{msg}");

    // A slow research run still gets its answer through after the timeout.
    let late = tool(&core, &ctx("grok", &ws), "mux_reply", json!({"ticket": ticket, "text": "sorry, took a while"})).await;
    assert_eq!(late["accepted"], true);
    assert_eq!(late["note"], "late reply delivered");
    let done = tool(&core, &ctx("cursor", &ws), "mux_get_reply", json!({"ticket": ticket, "wait_s": 1})).await;
    assert_eq!((done["status"].as_str(), done["answer"].as_str()), (Some("answered"), Some("sorry, took a while")));
    // ...but only once.
    let again = tool(&core, &ctx("grok", &ws), "mux_reply", json!({"ticket": ticket, "text": "again"})).await;
    assert_eq!(again["accepted"], false);
}

#[tokio::test]
async fn unconfigured_grok_stays_inbox_only() {
    let (core, ws) = test_core("inbox");
    let ticket = core.start_ticket(&ctx("cursor", &ws), None, "grok", "hello", None).unwrap();
    let t = core.wait_ticket(&ticket, 1).await.unwrap();
    assert_eq!(t["status"], "pending");
    let inbox = tool(&core, &ctx("grok", &ws), "mux_inbox", json!({})).await;
    assert_eq!(inbox.as_array().unwrap().len(), 1);
}

#[test]
fn url_validation() {
    assert!(webhook::validate_url("https://grok.example.com/automations/webhook/abc").is_ok());
    assert!(webhook::validate_url("http://127.0.0.1:8080/automations/webhook/abc").is_ok());
    assert!(webhook::validate_url("http://grok.example.com/automations/webhook/abc").is_err(), "plain http to a remote host");
    assert!(webhook::validate_url("https://grok.example.com/other/path").is_err());
    assert!(webhook::validate_url("https://grok.example.com/automations/webhook/").is_err());
    assert!(webhook::validate_url("https://u:p@grok.example.com/automations/webhook/abc").is_err());
    assert!(webhook::validate_url("not a url").is_err());
}

#[test]
fn status_masks_url_and_never_returns_key() {
    let (core, _) = test_core("status");
    configure(&core, "https://grok.example.com/automations/webhook/secret-routine-id-9f3a");
    let s = webhook::status(&core.vault, "grok").to_string();
    assert!(!s.contains(KEY));
    assert!(!s.contains("secret-routine-id"), "routine id must be masked: {s}");
    assert!(s.contains("9f3a"));
    assert!(s.contains("\"configured\":true"));
}

#[tokio::test]
async fn each_bot_wakes_through_its_own_webhook() {
    let (url_a, seen_a) = fake_grok(200, r#"{"runUuid":"run-a"}"#).await;
    let (url_b, seen_b) = fake_grok(200, r#"{"runUuid":"run-b"}"#).await;
    let (core, ws) = test_core("twobots");
    configure(&core, &url_a);
    let bot = core.vault.add_bot("Job Hunter").unwrap();
    assert_eq!(bot, "grok:job-hunter");
    core.vault.set_setting(&format!("webhook.{bot}.url"), &url_b).unwrap();
    core.vault.set_wake_kind(&bot, "webhook").unwrap();

    let cursor = ctx("cursor", &ws);
    let t_b = core.start_ticket(&cursor, None, "job-hunter", "status of the bot?", None).unwrap();
    let t_a = core.start_ticket(&cursor, None, "grok:grok", "x", None);
    assert!(t_a.is_err(), "no such id");
    core.wait_ticket(&t_b, 1).await.unwrap();

    assert_eq!(seen_a.lock().unwrap().len(), 0, "the other bot's webhook must not be hit");
    let reqs = seen_b.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].header("authorization"), Some("Bearer job-hunter-key"));
    let body: Value = serde_json::from_str(&reqs[0].body).unwrap();
    let c = body["context"].as_str().unwrap();
    assert!(c.contains("For: @grok:job-hunter"), "{c}");

    // Plain "grok" is ambiguous now.
    let e = core.start_ticket(&cursor, None, "grok", "hi", None).unwrap_err().to_string();
    assert!(e.contains("several Grok bots") && e.contains("grok:job-hunter"), "{e}");
}
