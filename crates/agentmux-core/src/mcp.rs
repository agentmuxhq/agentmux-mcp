use crate::core::{CallerCtx, Core};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;

const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "AgentMux connects your AI tools (Cursor, Grok, Claude Code, ChatGPT) over a local vault. \
Use mux_contacts to see who is reachable, mux_ask_peer to ask another agent a question (it returns their reply), \
mux_inbox/mux_reply to answer questions addressed to you, and mux_read_context / mux_publish_context for shared git diff, terminal output and notes. \
If mux_ask_peer comes back pending, keep calling mux_get_reply (it waits up to 110 s each time) until it is answered, unless the user asks you to stop. \
At the start of a turn, call mux_updates if you may be waiting on another agent: it lists replies and questions that arrived since you last checked. \
Conversations are grouped into sessions by topic: pass a short `topic` to mux_ask_peer (for example \"release plan\" or \"QA\"), \
reuse the same wording to continue that conversation later (mux_sessions lists the room's topics), and set new_session only when the user asks for a fresh one.";

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": desc,
           "inputSchema": {"type": "object", "properties": props, "required": required}})
}

pub fn tools() -> Vec<Value> {
    let room = json!({"type": "string", "description": "Room (product) id or name; defaults to your workspace's product, or one chosen from the conversation"});
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
            json!({"room": room, "text": {"type": "string"}, "attachments": {"type": "array", "description": "Images, audio, video or PDFs: [{path} for a file on this Mac, or {data: base64, name, mime}], each with an optional caption", "items": {"type": "object"}}}),
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
                "attachments": {"type": "array", "description": "Images, audio, video or PDFs: [{path} for a file on this Mac, or {data: base64, name, mime}], each with an optional caption", "items": {"type": "object"}},
                "topic": {"type": "string", "description": "Short name of what this conversation is about, e.g. \"release plan\" or \"QA\". Questions on the same topic go to the same session, even days later; a new topic gets its own session. Reuse names from mux_sessions"},
                "new_session": {"type": "boolean", "description": "Start a fresh session even if one on this topic exists. Only when the user asks for a new one"},
                "session": {"type": "string", "description": "Continue this exact session id. Without topic or session, you continue the session where you last talked with this agent"},
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
        tool(
            "mux_ask_all",
            "Ask everyone in a thread at once, like a group chat: each member is woken and the answers come back together. Pick the thread by `session` or `topic` (default: your current thread).",
            json!({"question": {"type": "string"}, "topic": {"type": "string"}, "session": {"type": "string"},
                   "wait_s": {"type": "integer", "description": "Seconds to wait for answers (default 60, max 110)"}, "attachments": {"type": "array", "description": "Images, audio, video or PDFs: [{path} for a file on this Mac, or {data: base64, name, mime}], each with an optional caption", "items": {"type": "object"}}}),
            &["question"],
        ),
        tool(
            "mux_attachment",
            "Get an attachment by id: images come back as images; other files as their path (or data, for cloud bots).",
            json!({"id": {"type": "string"}}),
            &["id"],
        ),
        tool(
            "mux_search",
            "Search AgentMux memory across every product: finds the threads where something was discussed (by words, thread names, summaries, and meaning when enabled), with their current state and the best snippets.",
            json!({"query": {"type": "string", "description": "What to look for, e.g. \"animation handoff\""}, "limit": {"type": "integer", "description": "Threads to return (default 5)"}}),
            &["query"],
        ),
        tool(
            "mux_brief",
            "Get the context of a thread: its contextual summary (goal, decisions and why, current state, open items, who, key artifacts) and recent messages. Find it by `query` (\"what happened to the animation handoff?\"), `topic`, `session`, or omit all for your current product's brief. Pass `question` to get an answer from that memory when a writer engine is on.",
            json!({"query": {"type": "string"}, "topic": {"type": "string"}, "session": {"type": "string"}, "room": room, "question": {"type": "string"}}),
            &[],
        ),
        tool(
            "mux_use_room",
            "File your current folder under a product room by name (creating it if needed). Questions you ask from this folder go there from now on. Use it when the user says which product this work belongs to.",
            json!({"name": {"type": "string", "description": "The product/room name, e.g. Animation"}}),
            &["name"],
        ),
        tool(
            "mux_rename_room",
            "Rename a room (a product). Everyone who took part is told the new name. Only when the user asks.",
            json!({"name": {"type": "string"}, "room": room}),
            &["name"],
        ),
        tool(
            "mux_sessions",
            "List the room's sessions (topics of conversation) with their last activity and participants, to pick a topic for mux_ask_peer.",
            json!({"room": room}),
            &[],
        ),
        tool("mux_inbox", "List questions other agents have asked you that are still unanswered.", json!({}), &[]),
        tool(
            "mux_reply",
            "Answer a question from your inbox.",
            json!({"ticket": {"type": "string"}, "text": {"type": "string"}, "code": {"type": "string"}, "attachments": {"type": "array", "description": "Images, audio, video or PDFs: [{path} for a file on this Mac, or {data: base64, name, mime}], each with an optional caption", "items": {"type": "object"}}}),
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
                    Ok(v) => {
                        let mut out = ok(&v);
                        // Multimodal agents on this Mac see attached images, not just their names.
                        if !crate::providers::remote_access(&ctx.agent) {
                            if let Some(atts) = v["attachments"].as_array() {
                                for b in crate::media::image_blocks(atts, 4) {
                                    out["content"].as_array_mut().unwrap().push(b);
                                }
                            }
                        }
                        out
                    }
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
                    rooms.push(json!({"id": r["id"], "title": r["title"], "folders": r["folders"],
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
            let (ids, desc) = store_attachments(core, ctx, a)?;
            let body = if desc.is_empty() { text.to_string() } else { format!("{text}\n{desc}") };
            let msg = core.vault.add_message(&sess, &room, &ctx.agent, Some("all"), "context_sync", &body, None, None)?;
            if let Some(mid) = msg["id"].as_str() {
                crate::media::link(&core.vault, &ids, mid)?;
            }
            core.emit("message", msg.clone());
            core.notify_thread(&sess, &room, &ctx.agent, None, &body)?;
            let _ = core.vault.export_markdown(&room);
            Ok(json!({"posted": true, "room": room, "attachments": ids}))
        }
        "mux_ask_peer" => {
            let to = arg_str(a, "to").unwrap_or("");
            let q = arg_str(a, "question").unwrap_or("");
            if q.trim().is_empty() {
                bail!("question is required");
            }
            let wait = a.get("wait_s").and_then(|v| v.as_u64()).unwrap_or(50);
            // A folder AgentMux has never seen gets its own room; tell the agent so it can ask the
            // user which product it belongs to.
            let root = crate::context::workspace_root(std::path::Path::new(&ctx.cwd));
            let new_folder = arg_str(a, "room").is_none() && core.in_project(ctx) && root != crate::config::home() && core.vault.folder_room(&root.to_string_lossy())?.is_none();
            let topic = arg_str(a, "topic").filter(|t| !t.trim().is_empty());
            let mut session = arg_str(a, "session").filter(|s| !s.is_empty()).map(String::from);
            let mut new_session = a.get("new_session").and_then(|v| v.as_bool()).unwrap_or(false);
            let local = !crate::providers::remote_access(&ctx.agent);
            // Which product. A room named by the agent (creating it if the user picked a new name),
            // or, for agents not running in a project folder, the router reading the conversation.
            let mut room_hint: Option<String> = None;
            if let Some(r) = arg_str(a, "room").filter(|r| !r.trim().is_empty()) {
                room_hint = Some(match core.vault.find_room(r)? {
                    Some(id) => id,
                    None if local => core.vault.create_room(r)?,
                    None => bail!("unknown room '{r}'"),
                });
            } else if session.is_none() && local && !core.in_project(ctx) {
                let recent = core.vault.setting(&format!("agent.{}.room", ctx.agent))?.filter(|r| core.vault.room(r).ok().flatten().is_some());
                if topic.is_some() || recent.is_none() {
                    match crate::router::route_product(&core.vault, &ctx.agent, topic, q, true).await? {
                        crate::router::ProductRoute::Use { room, .. } => room_hint = Some(room),
                        crate::router::ProductRoute::New { name } => {
                            room_hint = Some(match core.vault.find_room(&name)? {
                                Some(id) => id,
                                None => core.vault.create_room(&name)?,
                            })
                        }
                        crate::router::ProductRoute::Ask { options, suggested } => {
                            return Ok(json!({"status": "needs_choice", "choice": "product", "topic": topic, "products": options, "suggestedNewProduct": suggested,
                                "note": "AgentMux isn't sure which product this is about. Ask the user which one (or the suggested new name, or a name of their own), then call mux_ask_peer again with the same question and `room` set to that product's name."}));
                        }
                    }
                }
            }
            let room_arg = room_hint.clone();
            let room_arg = room_arg.as_deref().or(arg_str(a, "room"));
            match (topic, &session) {
                // A topic without a thread: the router decides, or hands the choice to the user.
                (Some(t), None) if !new_session => {
                    let (room, _) = core.resolve_room(ctx, room_arg)?;
                    match crate::router::route(&core.vault, &ctx.agent, &room, t, q, !crate::providers::remote_access(&ctx.agent)).await? {
                        crate::router::Route::Use { session: s, .. } => session = Some(s),
                        crate::router::Route::New => new_session = true,
                        crate::router::Route::Ask { options } => {
                            return Ok(json!({"status": "needs_choice", "topic": t, "options": options,
                                "note": "AgentMux isn't sure which thread this belongs to. Ask the user which one (show them the options), then call mux_ask_peer again with the same question and topic plus `session` set to their choice, or `new_session: true` for a new thread."}));
                        }
                    }
                }
                // The user answered an earlier "which thread?".
                (Some(t), Some(s)) => {
                    if let Some(room) = core.vault.session_room(s)? {
                        crate::router::resolve(&core.vault, &ctx.agent, &room, t, s)?;
                    }
                }
                (Some(t), None) => {
                    let (room, _) = core.resolve_room(ctx, room_arg)?;
                    crate::router::resolve(&core.vault, &ctx.agent, &room, t, "new")?;
                }
                _ => {}
            }
            let (ids, desc) = store_attachments(core, ctx, a)?;
            let question = if desc.is_empty() { q.to_string() } else { format!("{q}\n{desc}") };
            let opts = crate::core::AskOpts { topic: topic.map(String::from), new_session, attachments: ids };
            let ticket = core.start_ticket_with(ctx, room_arg, session.as_deref(), to, &question, arg_str(a, "then").map(String::from), &opts)?;
            let t = core.wait_ticket(&ticket, wait).await?;
            let mut view = ticket_view(&t);
            add_answer_attachments(core, &t, &mut view);
            if let Some(title) = core.vault.session_title(t["sessionId"].as_str().unwrap_or(""))? {
                view["thread"] = json!(title);
            }
            if new_folder {
                let rooms: Vec<String> = core
                    .vault
                    .room_ids()?
                    .iter()
                    .filter_map(|id| core.vault.room(id).ok().flatten())
                    .filter(|r| r["id"] != t["roomId"])
                    .filter_map(|r| r["title"].as_str().map(String::from))
                    .take(12)
                    .collect();
                if !rooms.is_empty() {
                    view["room_note"] = json!(format!(
                        "This folder is new to AgentMux, so it got its own room. If it belongs to an existing product ({}), ask the user and call mux_use_room with that name.",
                        rooms.join(", ")
                    ));
                }
            }
            Ok(view)
        }
        "mux_ask_all" => ask_all(core, ctx, a).await,
        "mux_attachment" => {
            let id = arg_str(a, "id").unwrap_or("");
            let att = crate::media::get(&core.vault, id)?.ok_or_else(|| anyhow::anyhow!("unknown attachment '{id}'"))?;
            if crate::providers::remote_access(&ctx.agent) {
                let sess = crate::media::session_of(&core.vault, id)?.unwrap_or_default();
                if !core.vault.last_pair_session_any(&sess, &ctx.agent)? {
                    bail!("that attachment isn't in a thread you took part in");
                }
                let bytes = std::fs::read(att["path"].as_str().unwrap_or(""))?;
                if bytes.len() > 10 * 1024 * 1024 {
                    bail!("that file is over 10 MB, too big to send to a cloud bot");
                }
                use base64::Engine as _;
                return Ok(json!({"id": id, "name": att["name"], "mime": att["mime"], "caption": att["caption"],
                                 "data": base64::engine::general_purpose::STANDARD.encode(bytes)}));
            }
            Ok(json!({"attachments": [att]}))
        }
        "mux_search" => {
            let scope = memory_scope(&ctx.agent);
            let limit = a.get("limit").and_then(|v| v.as_u64()).unwrap_or(5).clamp(1, 20) as usize;
            let hits = crate::memory::search(&core.vault, arg_str(a, "query").unwrap_or(""), &scope, limit).await?;
            Ok(json!({"results": hits, "note": if hits.is_empty() { "Nothing found. Try other words, or mux_sessions for this product's threads." } else { "Use mux_brief with a result's session for its full context." }}))
        }
        "mux_brief" => brief(core, ctx, a).await,
        "mux_use_room" => {
            let name = arg_str(a, "name").unwrap_or("");
            let (room, created) = core.use_room(ctx, name)?;
            Ok(json!({"room": room, "name": core.vault.room(&room)?.map(|r| r["title"].clone()), "created": created}))
        }
        "mux_rename_room" => {
            let (room, _) = core.resolve_room(ctx, arg_str(a, "room"))?;
            let name = core.rename_room(&room, arg_str(a, "name").unwrap_or(""), &ctx.agent)?;
            Ok(json!({"room": room, "name": name}))
        }
        "mux_get_reply" => {
            let id = arg_str(a, "ticket").unwrap_or("");
            let wait = a.get("wait_s").and_then(|v| v.as_u64()).unwrap_or(0);
            let t = core.wait_ticket(id, wait).await?;
            if t["from"] != ctx.agent.as_str() {
                bail!("ticket {id} was not asked by you");
            }
            let mut view = ticket_view(&t);
            add_answer_attachments(core, &t, &mut view);
            Ok(view)
        }
        "mux_register" => {
            core.vault.set_profile(&ctx.agent, arg_str(a, "name").unwrap_or(""), arg_str(a, "description"))?;
            core.emit("provider_connected", json!({"id": ctx.agent}));
            Ok(json!({"registered": true, "agent": ctx.agent, "name": core.vault.agent_name(&ctx.agent)}))
        }
        "mux_inbox" => Ok(json!(core.vault.inbox(&ctx.agent)?)),
        "mux_sessions" => {
            let (room, _) = core.resolve_room(ctx, arg_str(a, "room"))?;
            let title = core.vault.room(&room)?.map(|r| r["title"].clone()).unwrap_or(json!(room));
            Ok(json!({"room": room, "roomName": title, "sessions": core.vault.session_list(&room)?}))
        }
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
            let (ids, desc) = store_attachments(core, ctx, a)?;
            let body = if desc.is_empty() { text.to_string() } else { format!("{text}\n{desc}") };
            let accepted = core.finish_with(id, "answered", &body, arg_str(a, "code"), &ids)?;
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

/// Agents on this Mac search every product; a cloud bot only the threads it took part in.
fn memory_scope(agent: &str) -> crate::memory::Scope {
    if crate::providers::remote_access(agent) { crate::memory::Scope::Participant(agent.to_string()) } else { crate::memory::Scope::All }
}

async fn brief(core: &Arc<Core>, ctx: &CallerCtx, a: &Value) -> Result<Value> {
    use crate::memory;
    let scope = memory_scope(&ctx.agent);
    let mut others = vec![];
    // Which thread: explicit session, a topic in the caller's product, or the best search match.
    let session = if let Some(s) = arg_str(a, "session") {
        Some(s.to_string())
    } else if let Some(t) = arg_str(a, "topic") {
        let (room, _) = core.resolve_room(ctx, arg_str(a, "room"))?;
        core.vault.session_for_topic(&room, t)?
    } else if let Some(q) = arg_str(a, "query").or(arg_str(a, "question")) {
        let hits = memory::search(&core.vault, q, &scope, 4).await?;
        others = hits.iter().skip(1).map(|h| json!({"session": h["session"], "room": h["roomName"], "thread": h["thread"]})).collect();
        hits.first().and_then(|h| h["session"].as_str().map(String::from))
    } else {
        None
    };
    let Some(session) = session else {
        // No thread asked for: the product brief.
        let (room, _) = core.resolve_room(ctx, arg_str(a, "room"))?;
        let b = match memory::brief_of(&core.vault, &room)? {
            Some(b) => b,
            None => memory::brief(&core.vault, &room).await?,
        };
        return Ok(json!({"room": room, "brief": b}));
    };
    if let memory::Scope::Participant(agent) = &scope {
        let took_part: bool = core.vault.last_pair_session_any(&session, agent)?;
        if !took_part {
            anyhow::bail!("that thread isn't one you took part in");
        }
    }
    let room = core.vault.session_room(&session)?.ok_or_else(|| anyhow::anyhow!("unknown session '{session}'"))?;
    // A summary that's behind gets refreshed now, so the hand-over is current.
    let (summary, engine) = memory::summarize(&core.vault, &session).await?;
    let msgs = core.vault.messages(&session)?;
    let recent: Vec<Value> = msgs
        .iter()
        .rev()
        .filter(|m| m["type"] != "system")
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|m| json!({"from": m["sender"], "to": m["recipient"], "at": m["createdAt"], "text": memory::clip(m["text"].as_str().unwrap_or(""), 800)}))
        .collect();
    let room_v = core.vault.room(&room)?.unwrap_or(json!({}));
    let thread = room_v["sessions"].as_array().and_then(|ss| ss.iter().find(|x| x["id"] == session.as_str()).map(|x| x["title"].clone())).unwrap_or(json!(null));
    let mut out = json!({"room": room, "roomName": room_v["title"], "session": session, "thread": thread,
                         "summary": summary, "summaryBy": engine, "recent": recent, "otherMatches": others,
                         "attachments": crate::media::list_for_session(&core.vault, &session).unwrap_or_default()});
    if let Some(q) = arg_str(a, "question") {
        if crate::engines::Engine::load(&core.vault, "writer").enabled() {
            let ctx_text = format!("Thread: {} › {}\n\n{summary}\n\nRecent messages:\n{}", room_v["title"].as_str().unwrap_or(""), thread.as_str().unwrap_or(""), serde_json::to_string_pretty(&recent)?);
            match crate::engines::complete(&core.vault, "Answer the question from this thread's memory only. If the memory doesn't say, say so. Be brief and specific.", &format!("{ctx_text}\n\nQuestion: {q}")).await {
                Ok(ans) => out["answer"] = json!(ans),
                Err(e) => out["answerError"] = json!(e.to_string()),
            }
        } else {
            out["note"] = json!("No writer engine is on, so here is the thread's memory to answer from.");
        }
    }
    Ok(out)
}

/// Store a tool call's `attachments`, returning their ids and the line describing them.
fn store_attachments(core: &Arc<Core>, ctx: &CallerCtx, a: &Value) -> Result<(Vec<String>, String)> {
    let Some(list) = a.get("attachments").and_then(|v| v.as_array()).filter(|l| !l.is_empty()) else { return Ok((vec![], String::new())) };
    if list.len() > 10 {
        bail!("at most 10 attachments per message");
    }
    let local = !crate::providers::remote_access(&ctx.agent);
    let saved: Vec<Value> = list.iter().map(|x| crate::media::save(&core.vault, "", "", x, local)).collect::<Result<_>>()?;
    let ids = saved.iter().filter_map(|x| x["id"].as_str().map(String::from)).collect();
    Ok((ids, crate::media::describe(&saved)))
}

fn add_answer_attachments(core: &Arc<Core>, t: &Value, view: &mut Value) {
    if let Ok(atts) = crate::media::for_ticket_answer(&core.vault, t["id"].as_str().unwrap_or("")) {
        if !atts.is_empty() {
            view["attachments"] = json!(atts);
        }
    }
}

/// Ask every member of a thread at once and gather the answers.
async fn ask_all(core: &Arc<Core>, ctx: &CallerCtx, a: &Value) -> Result<Value> {
    let q = arg_str(a, "question").unwrap_or("");
    if q.trim().is_empty() {
        bail!("question is required");
    }
    let (room, latest) = core.resolve_room(ctx, None)?;
    let session = if let Some(s) = arg_str(a, "session") {
        s.to_string()
    } else if let Some(t) = arg_str(a, "topic") {
        core.vault.session_for_topic(&room, t)?.ok_or_else(|| anyhow::anyhow!("no thread on '{t}' in this product (see mux_sessions)"))?
    } else {
        let root = crate::context::workspace_root(std::path::Path::new(&ctx.cwd));
        core.vault.active_thread(&ctx.agent, &root.to_string_lossy())?.and_then(|t| t.1).unwrap_or(latest)
    };
    let members: Vec<String> = core.vault.session_members(&session)?.into_iter().filter(|m| *m != ctx.agent).collect();
    if members.is_empty() {
        bail!("nobody else is in this thread yet; ask someone with mux_ask_peer first");
    }
    let (ids, desc) = store_attachments(core, ctx, a)?;
    let question = if desc.is_empty() { q.to_string() } else { format!("{q}\n{desc}") };
    let wait = a.get("wait_s").and_then(|v| v.as_u64()).unwrap_or(60).min(110);
    let mut started = vec![];
    let mut results = vec![];
    for (i, m) in members.iter().enumerate() {
        let opts = crate::core::AskOpts { attachments: if i == 0 { ids.clone() } else { vec![] }, ..Default::default() };
        match core.start_ticket_with(ctx, None, Some(&session), m, &question, None, &opts) {
            Ok(t) => started.push((m.clone(), t)),
            Err(e) => results.push(json!({"to": m, "status": "not asked", "error": e.to_string()})),
        }
    }
    let mut set = tokio::task::JoinSet::new();
    for (m, t) in started {
        let c = core.clone();
        set.spawn(async move { (m, c.wait_ticket(&t, wait).await) });
    }
    while let Some(Ok((m, r))) = set.join_next().await {
        match r {
            Ok(t) => {
                let mut v = ticket_view(&t);
                v["to"] = json!(m);
                add_answer_attachments(core, &t, &mut v);
                results.push(v);
            }
            Err(e) => results.push(json!({"to": m, "status": "error", "error": e.to_string()})),
        }
    }
    Ok(json!({"session": session, "thread": core.vault.session_title(&session)?, "results": results}))
}
