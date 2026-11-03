//! `agentmux mcp --agent <id>`: the stdio MCP server agents launch. It relays JSON-RPC lines to the
//! daemon's socket. The daemon can restart under it (app update, app takeover, on/off switch), so
//! it reconnects instead of exiting: an agent's chat keeps working without restarting the agent.
//! Requests that were in flight when the daemon went away get an error asking to retry, never a hang.

use crate::{connect_or_spawn, hello};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// Id of the initialize the shim replays on a fresh connection; its reply isn't forwarded.
const REINIT_ID: &str = "agentmux-shim-reinit";

struct Conn {
    lines: Lines<BufReader<OwnedReadHalf>>,
    w: OwnedWriteHalf,
}

/// `patient`: a daemon was there a moment ago, so wait for it to come back before spawning one.
async fn open(agent: &str, init: Option<&Value>, patient: bool) -> Result<Conn> {
    let mut last = None;
    // A restarting daemon (app update, takeover) needs a moment to bind the socket again. Wait for
    // it rather than starting a second daemon that would race it; only spawn as a last resort.
    for attempt in 0..20 {
        let conn = if patient && attempt < 19 {
            tokio::net::UnixStream::connect(crate::config::socket_path()).await.map_err(anyhow::Error::from)
        } else {
            connect_or_spawn().await
        };
        match conn {
            Ok(s) => {
                let (r, mut w) = s.into_split();
                w.write_all(hello(agent).as_bytes()).await?;
                if let Some(init) = init {
                    let mut again = init.clone();
                    again["id"] = json!(REINIT_ID);
                    w.write_all(format!("{again}\n").as_bytes()).await?;
                }
                return Ok(Conn { lines: BufReader::new(r).lines(), w });
            }
            Err(e) if !patient || crate::config::is_paused() => return Err(e),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("AgentMux is not reachable")))
}

/// What the agent sees for a request AgentMux couldn't finish.
fn failure(id: &Value, method: &str, msg: &str) -> String {
    let v = if method == "tools/call" {
        json!({"jsonrpc": "2.0", "id": id, "result": {"isError": true, "content": [{"type": "text", "text": msg}]}})
    } else {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": msg}})
    };
    format!("{v}\n")
}

pub async fn run(agent: &str) -> Result<()> {
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    let mut init: Option<Value> = None;
    let mut conn = Some(open(agent, None, false).await?);
    // Requests sent to the daemon and not answered yet: id (as JSON text) → (id, method).
    let mut pending: HashMap<String, (Value, String)> = HashMap::new();

    loop {
        tokio::select! {
            line = stdin.next_line() => {
                let Some(line) = line? else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let method = msg["method"].as_str().unwrap_or("").to_string();
                if method == "initialize" {
                    init = Some(msg.clone());
                }
                let id = msg.get("id").filter(|i| !i.is_null() && !method.is_empty()).cloned();
                if conn.is_none() {
                    conn = open(agent, init.as_ref().filter(|_| method != "initialize"), true).await.ok();
                }
                let mut sent = match conn.as_mut() {
                    Some(c) => c.w.write_all(format!("{line}\n").as_bytes()).await.is_ok(),
                    None => false,
                };
                if !sent && conn.is_some() {
                    // The daemon died before we noticed: reconnect once and resend.
                    conn = open(agent, init.as_ref().filter(|_| method != "initialize"), true).await.ok();
                    if let Some(c) = conn.as_mut() {
                        sent = c.w.write_all(format!("{line}\n").as_bytes()).await.is_ok();
                    }
                }
                match (sent, id) {
                    (true, Some(id)) => {
                        pending.insert(id.to_string(), (id, method));
                    }
                    (true, None) => {}
                    (false, id) => {
                        conn = None;
                        if let Some(id) = id {
                            let why = if crate::config::is_paused() { "AgentMux is turned off. Turn it on from the menu bar icon, then try again." } else { "AgentMux isn't reachable right now. Try again in a moment." };
                            out.write_all(failure(&id, &method, why).as_bytes()).await?;
                            out.flush().await?;
                        }
                    }
                }
            }
            line = async { conn.as_mut().unwrap().lines.next_line().await }, if conn.is_some() => {
                match line {
                    Ok(Some(line)) => {
                        let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                        if msg["id"] == REINIT_ID {
                            continue;
                        }
                        if let Some(id) = msg.get("id") {
                            pending.remove(&id.to_string());
                        }
                        out.write_all(format!("{line}\n").as_bytes()).await?;
                        out.flush().await?;
                    }
                    _ => {
                        // The daemon went away (restart, update, turned off). Reconnect on the next request.
                        conn = None;
                        for (_, (id, method)) in pending.drain() {
                            let text = failure(&id, &method, "AgentMux restarted while this was running. Please try again (for a question you asked, check mux_updates first).");
                            out.write_all(text.as_bytes()).await?;
                        }
                        out.flush().await?;
                    }
                }
            }
        }
    }
    Ok(())
}
