use crate::core::{CallerCtx, Core};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;

const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "AgentMux connects your AI tools (Cursor, Grok, Claude Code, ChatGPT) over a local vault. \
Use mux_contacts to see who is reachable, mux_ask_peer to ask another agent a question (it returns their reply), \
mux_inbox/mux_reply to answer questions addressed to you, and mux_read_context / mux_publish_context for shared git diff, terminal output and notes. \
If mux_ask_peer comes back pending, keep calling mux_get_reply (it waits up to 110 s each time) until it is answered, unless the user asks you to stop. \
At the start of a turn, call mux_updates if you may be waiting on another agent: it lists replies and questions that arrived since you last checked.";

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": desc,
           "inputSchema": {"type": "object", "properties": props, "required": required}})
}

pub fn tools() -> Vec<Value> {
    let room = json!({"type": "string", "description": "Room id; defaults to the room for your current workspace"});
    vec![
        tool("mux_whoami", "Show which agent AgentMux thinks you are and your current room.", json!({}), &[]),
        tool("mux_contacts", "List the agents you can reach, whether they are connected, and how they are woken.", json!({}), &[]),
        tool("mux_list_rooms", "List bridge rooms (one per workspace) with participants.", json!({}), &[]),
        tool(
            "mux_read_context",
            "Read a room's shared git diff, terminal output, notes and recent messages.",
            json!({"room": room}),
            &[],
        ),
        tool(
            "mux_post",
            "Post a note into the room's current session for all agents to see.",
            json!({"room": room, "text": {"type": "string"}}),
            &["text"],
        ),
        tool(
            "mux_ask_peer",
            "Ask another agent a question. AgentMux wakes them, and this returns their reply (or a pending ticket if they take longer than wait_s; then keep calling mux_get_reply until it is answered).",
            json!({
                "to": {"type": "string", "description": "Agent id or name, e.g. cursor, claude, chatgpt, grok:job-hunter (see mux_contacts)"},
                "question": {"type": "string"},
                "room": room,
                "wait_s": {"type": "integer", "description": "Seconds to wait for the reply (default 50, max 110)"},
                "session": {"type": "string", "description": "Continue this session. By default a new session starts unless you asked this agent something in the last 30 minutes"},
                "then": {"type": "string", "description": "Optionally forward the answer to this agent next"}
            }),
            &["to", "question"],
        ),
        tool(
            "mux_get_reply",
            "Fetch (optionally waiting for) the reply to an earlier mux_ask_peer ticket. If it is still pending, call this again with wait_s up to 110 until it is answered, unless the user asks you to stop.",
            json!({"ticket": {"type": "string"}, "wait_s": {"type": "integer"}}),
            &["ticket"],
        ),
        tool(
            "mux_register",
            "Tell AgentMux your display name and a one-line description of what you do, so other agents can find and address you.",
            json!({"name": {"type": "string"}, "description": {"type": "string"}}),
            &["name"],
        ),
        tool(
            "mux_updates",
            "What's new for you since you last checked: answers to your questions (including late ones) and new questions addressed to you.",
            json!({"mark_read": {"type": "boolean", "description": "Advance your read cursor (default true)"}}),
            &[],
        ),
        tool("mux_inbox", "List questions other agents have asked you that are still unanswered.", json!({}), &[]),
        tool(
            "mux_reply",
            "Answer a question from your inbox.",
            json!({"ticket": {"type": "string"}, "text": {"type": "string"}, "code": {"type": "string"}}),
            &["ticket", "text"],
        ),
        tool(
            "mux_publish_context",
            "Publish shared context for a room: terminal output, notes, or a diff.",
            json!({"room": room, "kind": {"type": "string", "enum": ["notes", "terminal", "diff"]}, "content": {"type": "string"}}),
            &["kind", "content"],
        ),
    ]
}

fn ok(v: &Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]})
}

fn err(msg: &str) -> Value {
    json!({"isError": true, "content": [{"type": "text", "text": msg}]})
}

fn arg_str<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(|v| v.as_str())
}

/// Handle one JSON-RPC message. Returns None for notifications.
pub async fn handle(core: &Arc<Core>, ctx: &CallerCtx, req: Value) -> Option<Value> {
    handle_scoped(core, ctx, req, None).await
}

/// Like `handle`, but when `allowed` is set only those tools are listed and callable.
/// Used for connections that arrive over the (tunnelled) HTTP gateway.
pub async fn handle_scoped(core: &Arc<Core>, ctx: &CallerCtx, req: Value, allowed: Option<&[&str]>) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));
    let reply = |result: Value| id.clone().map(|id| json!({"jsonrpc": "2.0", "id": id, "result": result}));

    match method {
        "initialize" => {
            core.on_initialize(&ctx.agent);
            let want = params["protocolVersion"].as_str().unwrap_or("");
            let ver = if SUPPORTED.contains(&want) { want } else { SUPPORTED[0] };
            reply(json!({
                "protocolVersion": ver,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "agentmux", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS
            }))
        }
        "ping" => reply(json!({})),
        "tools/list" => {
            let all = tools();
            let list: Vec<Value> = match allowed {
                Some(a) => all.into_iter().filter(|t| a.contains(&t["name"].as_str().unwrap_or(""))).collect(),
                None => all,
            };
            reply(json!({"tools": list}))
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let _ = core.vault.touch_provider(&ctx.agent);
            let out = if allowed.is_some_and(|a| !a.contains(&name.as_str())) {
                err(&format!("tool '{name}' is not available on this connection"))
            } else {
                match call_tool(core, ctx, &name, &args).await {
                    Ok(v) => ok(&v),
                    Err(e) => err(&e.to_string()),
                }
            };
            reply(out)
        }
        m if m.starts_with("notifications/") => None,
        _ => id.map(|id| json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {method}")}})),
    }
}

async fn call_tool(core: &Arc<Core>, ctx: &CallerCtx, name: &str, a: &Value) -> Result<Value> {
    match name {
        "mux_whoami" => {
            let (room, _) = core.resolve_room(ctx, None)?;
            let me = core.vault.providers()?.into_iter().find(|p| p["id"] == ctx.agent.as_str()).unwrap_or(json!({}));
            Ok(json!({"agent": ctx.agent, "name": core.vault.agent_name(&ctx.agent), "registered": me["registered"], "description": me["description"], "cwd": ctx.cwd, "room": room}))
        }
        "mux_contacts" => {
            let mut ps = core.vault.providers()?;
            for p in ps.iter_mut() {
                p["you"] = json!(p["id"] == ctx.agent.as_str());
                p["connected"] = json!(p["connectedAt"].is_string());
            }
            Ok(json!(ps))
        }
        "mux_list_rooms" => {
            let mut rooms = vec![];
            for id in core.vault.room_ids()? {
                if let Some(r) = core.vault.room(&id)? {
                    rooms.push(json!({"id": r["id"], "title": r["title"], "workspace": r["workspacePath"],
                                      "branch": r["branch"], "participants": r["participants"]}));
                }
            }
            Ok(json!(rooms))
        }
        "mux_read_context" => {
            let (room, sess) = core.resolve_room(ctx, arg_str(a, "room"))?;
            let mut r = core.vault.room(&room)?.unwrap_or(json!({}));
            let msgs = core.vault.messages(&sess)?;
            let start = msgs.len().saturating_sub(30);
            r["recentMessages"] = json!(msgs[start..]);
            Ok(r)
        }
        "mux_post" => {
            let text = arg_str(a, "text").unwrap_or("");
            if text.trim().is_empty() {
                bail!("text is required");
            }
            let (room, sess) = core.resolve_room(ctx, arg_str(a, "room"))?;
            let msg = core.vault.add_message(&sess, &room, &ctx.agent, Some("all"), "context_sync", text, None, None)?;
            core.emit("message", msg.clone());
            let _ = core.vault.export_markdown(&room);
            Ok(json!({"posted": true, "room": room}))
        }
        "mux_ask_peer" => {
            let to = arg_str(a, "to").unwrap_or("");
            let q = arg_str(a, "question").unwrap_or("");
            if q.trim().is_empty() {
                bail!("question is required");
            }
            let wait = a.get("wait_s").and_then(|v| v.as_u64()).unwrap_or(50);
            let ticket = core.start_ticket_in(ctx, arg_str(a, "room"), arg_str(a, "session"), to, q, arg_str(a, "then").map(String::from))?;
            let t = core.wait_ticket(&ticket, wait).await?;
            Ok(ticket_view(&t))
        }
        "mux_get_reply" => {
            let id = arg_str(a, "ticket").unwrap_or("");
            let wait = a.get("wait_s").and_then(|v| v.as_u64()).unwrap_or(0);
            let t = core.wait_ticket(id, wait).await?;
            if t["from"] != ctx.agent.as_str() {
                bail!("ticket {id} was not asked by you");
            }
            Ok(ticket_view(&t))
        }
        "mux_register" => {
            core.vault.set_profile(&ctx.agent, arg_str(a, "name").unwrap_or(""), arg_str(a, "description"))?;
            core.emit("provider_connected", json!({"id": ctx.agent}));
            Ok(json!({"registered": true, "agent": ctx.agent, "name": core.vault.agent_name(&ctx.agent)}))
        }
        "mux_inbox" => Ok(json!(core.vault.inbox(&ctx.agent)?)),
        "mux_updates" => {
            let mark = a.get("mark_read").and_then(|v| v.as_bool()).unwrap_or(true);
            let items = core.vault.updates_for(&ctx.agent, mark)?;
            Ok(json!({"count": items.len(), "updates": items,
                      "note": if items.is_empty() { "Nothing new." } else { "Questions addressed to you: answer them with mux_reply." }}))
        }
        "mux_reply" => {
            let id = arg_str(a, "ticket").unwrap_or("");
            let text = arg_str(a, "text").unwrap_or("");
            let Some(t) = core.vault.ticket(id)? else { bail!("unknown ticket {id}") };
            if t["to"] != ctx.agent.as_str() {
                bail!("ticket {id} is not addressed to you");
            }
            let accepted = core.finish(id, "answered", text, arg_str(a, "code"))?;
            let late = t["status"] == "expired";
            Ok(json!({"accepted": accepted, "note": match (accepted, late) {
                (true, true) => "late reply delivered",
                (true, false) => "reply delivered",
                _ => "ticket was already closed",
            }}))
        }
        "mux_publish_context" => {
            let kind = arg_str(a, "kind").unwrap_or("");
            if !matches!(kind, "notes" | "terminal" | "diff") {
                bail!("kind must be notes, terminal or diff");
            }
            let (room, _) = core.resolve_room(ctx, arg_str(a, "room"))?;
            core.vault.set_context(&room, kind, arg_str(a, "content").unwrap_or(""))?;
            core.emit("context", json!({"room": room, "kind": kind}));
            let _ = core.vault.export_markdown(&room);
            Ok(json!({"published": kind, "room": room}))
        }
        _ => bail!("unknown tool '{name}'"),
    }
}

fn ticket_view(t: &Value) -> Value {
    match t["status"].as_str() {
        Some("answered") => json!({"status": "answered", "from": t["to"], "ticket": t["id"], "answer": t["answer"]}),
        Some("failed") => json!({"status": "failed", "ticket": t["id"], "error": t["answer"]}),
        Some("expired") => json!({"status": "expired", "ticket": t["id"], "error": t["answer"],
                                  "note": "No answer yet, but a late answer is still accepted. Call mux_get_reply with this ticket later."}),
        _ => json!({"status": t["status"], "ticket": t["id"],
                    "note": "Not answered yet. Call mux_get_reply with this ticket later."}),
    }
}
