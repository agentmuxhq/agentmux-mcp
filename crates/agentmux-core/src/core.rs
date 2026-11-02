use crate::{config, context, vault::Vault, wakers, webhook};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify};

/// Who is calling: set once per connection from the shim's hello line.
#[derive(Clone, Debug)]
pub struct CallerCtx {
    pub agent: String,
    pub cwd: String,
    pub hop: i64,
}

pub struct Core {
    pub vault: Arc<Vault>,
    notifiers: Mutex<HashMap<String, Arc<Notify>>>,
    events: broadcast::Sender<Value>,
}

impl Core {
    pub fn new() -> Result<Arc<Self>> {
        Ok(Self::with_vault(Arc::new(Vault::open()?)))
    }

    pub fn with_vault(vault: Arc<Vault>) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Core { vault, notifiers: Mutex::new(HashMap::new()), events })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub fn emit(&self, kind: &str, data: Value) {
        let _ = self.events.send(json!({"kind": kind, "data": data}));
    }

    fn notifier(&self, ticket: &str) -> Arc<Notify> {
        self.notifiers.lock().unwrap().entry(ticket.to_string()).or_default().clone()
    }

    pub fn on_initialize(&self, agent: &str) {
        if self.vault.mark_connected(agent).is_ok() {
            self.emit("provider_connected", json!({"id": agent}));
        }
    }

    /// Turn what a caller typed (`grok:job-hunter`, `job-hunter`, `Job Hunter`, `grok`) into an agent id.
    pub fn resolve_agent(&self, raw: &str) -> Result<String> {
        let want = raw.trim().trim_start_matches('@');
        let providers = self.vault.providers()?;
        let list = |ps: &[&Value]| ps.iter().map(|p| format!("{} ({})", p["id"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", ");
        let bots: Vec<&Value> = providers.iter().filter(|p| p["family"] == "grok").collect();
        if want == "grok" && bots.len() > 1 {
            // "grok" alone is ambiguous. The original bot is addressed by its name, added bots by name or id.
            let shown = bots
                .iter()
                .map(|p| {
                    let (id, name) = (p["id"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""));
                    if id == "grok" { format!("\"{name}\"") } else { format!("\"{name}\" ({id})") }
                })
                .collect::<Vec<_>>()
                .join(", ");
            bail!("several Grok bots are connected: {shown}. Ask again using one bot's name (or id)");
        }
        if let Some(p) = providers.iter().find(|p| p["id"] == want) {
            return Ok(p["id"].as_str().unwrap_or("").to_string());
        }
        let key = config::slug(want);
        let hits: Vec<&Value> = providers
            .iter()
            .filter(|p| {
                let id = p["id"].as_str().unwrap_or("");
                !key.is_empty()
                    && (config::slug(p["name"].as_str().unwrap_or("")) == key || id.strip_prefix("grok:") == Some(key.as_str()))
            })
            .collect();
        match hits.len() {
            1 => Ok(hits[0]["id"].as_str().unwrap_or("").to_string()),
            0 => bail!("unknown agent '{want}'. Reachable: {}", list(&providers.iter().collect::<Vec<_>>())),
            _ => bail!("'{want}' matches several agents: {} — use the exact id", list(&hits)),
        }
    }

    /// Room + latest session for an explicit room id, or for the caller's workspace.
    pub fn resolve_room(&self, ctx: &CallerCtx, explicit: Option<&str>) -> Result<(String, String)> {
        if let Some(r) = explicit.filter(|r| !r.is_empty()) {
            if self.vault.room(r)?.is_some() {
                let sess = self.vault.latest_session(r)?.ok_or_else(|| anyhow!("room {r} has no session"))?;
                return Ok((r.to_string(), sess));
            }
            bail!("unknown room '{r}' (see mux_list_rooms)");
        }
        let root = context::workspace_root(Path::new(&ctx.cwd));
        let (branch, _) = context::capture_git(&root);
        let (room, sess) = self.vault.ensure_room(&root.to_string_lossy(), &branch)?;
        if self.vault.is_agent(&ctx.agent) {
            self.vault.add_member(&room, &ctx.agent)?;
        }
        Ok((room, sess))
    }

    /// Create a ticket, record the request, and start waking the recipient in the background.
    pub fn start_ticket(
        self: &Arc<Self>,
        ctx: &CallerCtx,
        room_hint: Option<&str>,
        to: &str,
        question: &str,
        then: Option<String>,
    ) -> Result<String> {
        self.start_ticket_in(ctx, room_hint, None, to, question, then)
    }

    /// Which session a new question belongs to. A session is one conversation: the latest one is
    /// reused only if it is still empty or the same two agents talked there recently; otherwise a
    /// fresh session starts, so unrelated exchanges in the same workspace don't pile together.
    fn pick_session(&self, room: &str, latest: &str, from: &str, to: &str) -> Result<String> {
        if self.vault.session_message_count(latest)? == 0 {
            return Ok(latest.to_string());
        }
        let since = (chrono::Utc::now() - chrono::Duration::minutes(config::SESSION_CONTINUE_MINUTES)).to_rfc3339();
        if self.vault.pair_active_since(latest, from, to, &since)? {
            return Ok(latest.to_string());
        }
        let who = |id: &str| if id == "user" { "You".to_string() } else { self.vault.agent_name(id) };
        let title = format!("{} ↔ {}", who(from), who(to));
        self.vault.new_session(room, &title)
    }

    /// `start_ticket` with an explicit session to continue (the UI's open session, a handoff chain).
    pub fn start_ticket_in(
        self: &Arc<Self>,
        ctx: &CallerCtx,
        room_hint: Option<&str>,
        session_hint: Option<&str>,
        to: &str,
        question: &str,
        then: Option<String>,
    ) -> Result<String> {
        let to = self.resolve_agent(to)?;
        let to = to.as_str();
        if to == ctx.agent {
            bail!("an agent cannot ask itself");
        }
        let then = match then {
            Some(t) if !t.is_empty() && t != "none" => {
                let t = self.resolve_agent(&t).map_err(|e| anyhow!("invalid 'then' target: {e}"))?;
                if t == to {
                    bail!("invalid 'then' target '{t}'");
                }
                Some(t)
            }
            _ => None,
        };
        let hop = ctx.hop + 1;
        if hop > config::MAX_HOPS {
            bail!("hop limit ({}) reached: this question was itself triggered by a chain of asks", config::MAX_HOPS);
        }
        if self.vault.open_ticket_between(&ctx.agent, to)? {
            bail!("@{} already has an open question to @{to}; wait for it or use mux_get_reply", ctx.agent);
        }
        let session_room = match session_hint.filter(|s| !s.is_empty()) {
            Some(s) => Some(self.vault.session_room(s)?.ok_or_else(|| anyhow!("unknown session '{s}'"))?),
            None => None,
        };
        let (room, latest) = self.resolve_room(ctx, room_hint.or(session_room.as_deref()))?;
        let sess = match (session_hint.filter(|s| !s.is_empty()), session_room) {
            (Some(s), Some(r)) if r == room => s.to_string(),
            (Some(s), _) => bail!("session '{s}' is not in room '{room}'"),
            _ => self.pick_session(&room, &latest, &ctx.agent, to)?,
        };
        self.vault.add_member(&room, to)?;

        // Snapshot the live git state so the peer sees what the caller sees.
        if let Some(r) = self.vault.room(&room)? {
            let ws = r["workspacePath"].as_str().unwrap_or("");
            let (branch, diff) = context::capture_git(Path::new(ws));
            if !diff.is_empty() {
                self.vault.set_context(&room, "diff", &diff)?;
            }
            if !branch.is_empty() {
                self.vault.ensure_room(ws, &branch)?;
            }
        }

        let ticket = self.vault.create_ticket(&sess, &room, &ctx.agent, to, question, hop, then.as_deref())?;
        let msg = self.vault.add_message(&sess, &room, &ctx.agent, Some(to), "a2a_request", question, None, Some(&ticket))?;
        self.emit("message", msg);
        self.emit("ticket", json!({"id": ticket, "status": "pending", "from": ctx.agent, "to": to, "room": room}));
        let _ = self.vault.export_markdown(&room);

        let core = self.clone();
        let t = ticket.clone();
        tokio::spawn(async move { core.run_ticket(t).await });
        Ok(ticket)
    }

    async fn run_ticket(self: Arc<Self>, ticket_id: String) {
        let Ok(Some(t)) = self.vault.ticket(&ticket_id) else { return };
        let to = t["to"].as_str().unwrap_or("").to_string();
        let from = t["from"].as_str().unwrap_or("").to_string();
        let kind = self
            .vault
            .providers()
            .ok()
            .and_then(|ps| ps.into_iter().find(|p| p["id"] == to.as_str()))
            .and_then(|p| p["wakeKind"].as_str().map(String::from))
            .unwrap_or_else(|| "inbox".into());
        if kind == "webhook" {
            self.wake_webhook(&ticket_id, &t).await;
            return;
        }
        if kind != "cli" {
            // Inbox-only: the recipient picks this up via mux_inbox on its next turn.
            return;
        }
        let room = t["roomId"].as_str().unwrap_or("").to_string();
        let r = self.vault.room(&room).ok().flatten().unwrap_or(json!({}));
        let ws = r["workspacePath"].as_str().unwrap_or(".").to_string();
        let allow = r["allowEdits"].as_bool().unwrap_or(false);
        let _ = self.vault.set_ticket_status(&ticket_id, "running");
        self.emit("ticket", json!({"id": ticket_id, "status": "running", "from": from, "to": to, "room": room}));

        let prompt = build_prompt(&self.vault, &t, &r, allow);
        let env = vec![
            ("AGENTMUX_TICKET".to_string(), ticket_id.clone()),
            ("AGENTMUX_HOP".to_string(), t["hop"].as_i64().unwrap_or(1).to_string()),
            ("AGENTMUX_AGENT".to_string(), to.clone()),
        ];
        match wakers::run_wake(&to, &prompt, &ws, allow, &env).await {
            Ok(text) => {
                let _ = self.finish(&ticket_id, "answered", &text, None);
            }
            Err(e) => {
                let _ = self.finish(&ticket_id, "failed", &e.to_string(), None);
            }
        }
    }

    /// Start a Grok routine run. The answer comes back asynchronously via `mux_reply`.
    async fn wake_webhook(self: &Arc<Self>, ticket_id: &str, t: &Value) {
        let (to, from, room_id) = (
            t["to"].as_str().unwrap_or("").to_string(),
            t["from"].as_str().unwrap_or("").to_string(),
            t["roomId"].as_str().unwrap_or("").to_string(),
        );
        let (url, key) = match webhook::load(&self.vault, &to) {
            Ok(c) => c,
            Err(e) => {
                let _ = self.finish(ticket_id, "failed", &e.to_string(), None);
                return;
            }
        };
        let room = self.vault.room(&room_id).ok().flatten().unwrap_or(json!({}));
        let diff = self.vault.context(&room_id, "diff").unwrap_or_default();
        let notes = self.vault.context(&room_id, "notes").unwrap_or_default();
        let ctx = webhook::build_context(t, &room, &diff, &notes);
        match webhook::post(&url, &key, &ctx).await {
            Ok(run) => {
                let _ = self.vault.set_ticket_status(ticket_id, "running");
                self.emit("ticket", json!({"id": ticket_id, "status": "running", "from": from, "to": to, "room": room_id, "run": run}));
                // The webhook only starts a run, so give the routine time to call mux_reply.
                let (core, tid) = (self.clone(), ticket_id.to_string());
                let secs = config::webhook_timeout_secs();
                let name = self.vault.agent_name(&to);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    let msg = format!(
                        "{name} accepted the webhook{} but has not called mux_reply within {secs}s. A late answer will still be delivered here. If none arrives, check that the routine's instructions tell it to call mux_reply and that AgentMux is connected in Grok Bot.",
                        run.map(|r| format!(" (run {r})")).unwrap_or_default()
                    );
                    let _ = core.finish(&tid, "expired", &msg, None);
                });
            }
            Err(e) => {
                let _ = self.finish(ticket_id, "failed", &e.to_string(), None);
            }
        }
    }

    /// Record the answer (first answer wins), notify waiters, and run any `then` handoff.
    pub fn finish(self: &Arc<Self>, ticket_id: &str, status: &str, text: &str, code: Option<&str>) -> Result<bool> {
        if !self.vault.answer_ticket(ticket_id, status, text)? {
            return Ok(false);
        }
        let t = self.vault.ticket(ticket_id)?.ok_or_else(|| anyhow!("ticket vanished"))?;
        let (to, from, room, sess) = (
            t["to"].as_str().unwrap_or("").to_string(),
            t["from"].as_str().unwrap_or("").to_string(),
            t["roomId"].as_str().unwrap_or("").to_string(),
            t["sessionId"].as_str().unwrap_or("").to_string(),
        );
        let kind = if status == "answered" { "a2a_reply" } else { "system" };
        let sender = if status == "answered" { to.as_str() } else { "agentmux" };
        let body = match status {
            "answered" => text.to_string(),
            "expired" => format!("@{to} has not answered yet: {text}"),
            _ => format!("@{to} could not answer: {text}"),
        };
        let msg = self.vault.add_message(&sess, &room, sender, Some(&from), kind, &body, code, Some(ticket_id))?;
        let _ = self.vault.touch_provider(&to);
        self.emit("message", msg);
        self.emit("ticket", json!({"id": ticket_id, "status": status, "from": from, "to": to, "room": room}));
        let _ = self.vault.export_markdown(&room);
        self.notifier(ticket_id).notify_waiters();

        if status == "answered" {
            if let Some(next) = t["then"].as_str() {
                let ws = self.vault.room(&room)?.and_then(|r| r["workspacePath"].as_str().map(String::from)).unwrap_or_default();
                let ctx = CallerCtx { agent: to.clone(), cwd: ws, hop: t["hop"].as_i64().unwrap_or(1) };
                let q = format!(
                    "@{from} asked @{to}: {}\n\n@{to} answered:\n{text}\n\nPlease review/continue from here.",
                    t["question"].as_str().unwrap_or("")
                );
                if let Err(e) = self.start_ticket_in(&ctx, Some(&room), Some(&sess), next, &q, None) {
                    tracing::warn!("handoff to @{next} failed: {e}");
                }
            }
        }
        Ok(true)
    }

    /// Wait up to `wait_s` for a ticket to leave pending/running; returns its final or current state.
    pub async fn wait_ticket(&self, ticket_id: &str, wait_s: u64) -> Result<Value> {
        let n = self.notifier(ticket_id);
        let deadline = Instant::now() + Duration::from_secs(wait_s.min(110));
        loop {
            let t = self.vault.ticket(ticket_id)?.ok_or_else(|| anyhow!("unknown ticket {ticket_id}"))?;
            if !matches!(t["status"].as_str(), Some("pending") | Some("running")) {
                return Ok(t);
            }
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Ok(t);
            }
            // Poll at least once a second: notify_waiters() stores no permit.
            let _ = tokio::time::timeout(rem.min(Duration::from_secs(1)), n.notified()).await;
        }
    }
}

fn build_prompt(vault: &Vault, t: &Value, room: &Value, allow_edits: bool) -> String {
    let rid = t["roomId"].as_str().unwrap_or("");
    let diff = vault.context(rid, "diff").unwrap_or_default();
    let notes = vault.context(rid, "notes").unwrap_or_default();
    let terminal = vault.context(rid, "terminal").unwrap_or_default();
    let mut p = format!(
        "You are @{to}. AgentMux woke you because @{from} asked you a question in room \"{title}\" (workspace {ws}, branch {branch}).\n\nQuestion from @{from}:\n{q}\n",
        to = t["to"].as_str().unwrap_or(""),
        from = t["from"].as_str().unwrap_or(""),
        title = room["title"].as_str().unwrap_or(""),
        ws = room["workspacePath"].as_str().unwrap_or(""),
        branch = room["branch"].as_str().unwrap_or(""),
        q = t["question"].as_str().unwrap_or(""),
    );
    if !diff.is_empty() {
        p.push_str(&format!("\nShared git diff:\n```diff\n{diff}\n```\n"));
    }
    if !terminal.is_empty() {
        p.push_str(&format!("\nShared terminal output:\n```\n{terminal}\n```\n"));
    }
    if !notes.is_empty() {
        p.push_str(&format!("\nShared notes:\n{notes}\n"));
    }
    p.push_str("\nAnswer directly and concisely. Your final message is delivered back to the asker as the reply.");
    if !allow_edits {
        p.push_str(" Do not modify any files.");
    }
    p
}
