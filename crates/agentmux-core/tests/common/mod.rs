//! Shared test helpers: a temp vault and core, and a fake JSON HTTP server that can play Ollama,
//! an OpenAI-compatible API, or a jeff-serve.
#![allow(dead_code)]

use agentmux_core::{CallerCtx, Core, Vault};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A fresh vault, core and a workspace folder outside any git repo.
pub fn setup(name: &str) -> (Arc<Core>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("agentmux-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let vault = Arc::new(Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap());
    (Core::with_vault(vault), dir.join("ws"))
}

pub fn ctx(agent: &str, cwd: &PathBuf) -> CallerCtx {
    CallerCtx { agent: agent.into(), cwd: cwd.to_string_lossy().into(), hop: 0 }
}

pub type Handler = Arc<dyn Fn(&str, &Value) -> (u16, Value) + Send + Sync>;

/// Serve JSON: `handler(path, request body) -> (status, response body)`. Returns the base URL and
/// the log of (path, body) received.
pub async fn fake_server(handler: Handler) -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = l.accept().await else { return };
            let (seen, handler) = (s2.clone(), handler.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 65536];
                let (path, body) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    let Some(i) = text.find("\r\n\r\n") else { continue };
                    let len: usize = text[..i]
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                        .unwrap_or(0);
                    if buf.len() >= i + 4 + len {
                        let path = text.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("/").to_string();
                        let body: Value = serde_json::from_slice(&buf[i + 4..i + 4 + len]).unwrap_or(Value::Null);
                        break (path, body);
                    }
                };
                seen.lock().unwrap().push((path.clone(), body.clone()));
                let (status, resp) = handler(&path, &body);
                let out = resp.to_string();
                let _ = sock
                    .write_all(format!("HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}", out.len()).as_bytes())
                    .await;
            });
        }
    });
    (url, seen)
}

/// Ask `to` something and answer it, returning the ticket.
pub fn exchange(core: &Arc<Core>, from: &str, cwd: &PathBuf, to: &str, topic: Option<&str>, q: &str, answer: &str) -> String {
    let opts = agentmux_core::AskOpts { topic: topic.map(String::from), ..Default::default() };
    let t = core.start_ticket_with(&ctx(from, cwd), None, None, to, q, None, &opts).unwrap();
    core.finish(&t, "answered", answer, None).unwrap();
    t
}

pub fn session_of(core: &Core, ticket: &str) -> String {
    core.vault.ticket(ticket).unwrap().unwrap()["sessionId"].as_str().unwrap().to_string()
}

/// Call an MCP tool as `agent` and return the decoded JSON result (or the error text).
pub async fn tool(core: &Arc<Core>, agent: &str, cwd: &PathBuf, name: &str, args: Value) -> Result<(Value, Value), String> {
    let req = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let resp = agentmux_core::mcp::handle(core, &ctx(agent, cwd), req).await.unwrap();
    let r = &resp["result"];
    let text = r["content"][0]["text"].as_str().unwrap_or("").to_string();
    if r["isError"] == true {
        return Err(text);
    }
    Ok((serde_json::from_str(&text).unwrap_or(Value::String(text)), r.clone()))
}
