use crate::{config, context, vault::Vault, wakers, webhook};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify};

/// Who is calling: set once per connection from the shim's hello line.
/// How a question chooses its session (see `Core::pick_session`).
#[derive(Clone, Debug, Default)]
pub struct AskOpts {
    /// What the conversation is about, e.g. "release plan". Same topic → same session.
    pub topic: Option<String>,
    /// Start a fresh session even if one on this topic exists.
    pub new_session: bool,
    /// Attachments already stored (`media::save`), linked to the question's message.
    pub attachments: Vec<String>,
}

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

    /// A thread is a group chat: everyone who took part hears about each new question, answer or
    /// note, except the two agents it's between (they already have it). Shown in mux_updates and
    /// the Claude Code hook.
    pub fn notify_thread(&self, session: &str, room: &str, actor: &str, partner: Option<&str>, text: &str) -> Result<()> {
        let members = self.vault.session_members(session)?;
        if members.len() <= 2 {
            return Ok(());
        }
        let title = self.vault.session_title(session)?.unwrap_or_default();
        let room_name = self.vault.room(room)?.and_then(|r| r["title"].as_str().map(String::from)).unwrap_or_default();
        let who = |id: &str| if id == "user" { "You".to_string() } else { self.vault.agent_name(id) };
        let line = format!(
            "{room_name} › {title}: {}{}: {}",
            who(actor),
            partner.map(|p| format!(" → {}", who(p))).unwrap_or_default(),
            crate::memory::clip(text, 300)
        );
        for m in members.iter().filter(|m| m.as_str() != actor && Some(m.as_str()) != partner) {
            self.vault.add_notice(m, room, &line)?;
        }
        Ok(())
    }

    /// Keep memory current while `owns()` (this process owns the socket): embed new messages, and
    /// refresh the summaries and briefs of threads that changed and went quiet.
    pub async fn memory_worker(self: Arc<Self>, owns: impl Fn() -> bool + Send + Sync + 'static) {
        tokio::time::sleep(Duration::from_secs(15)).await;
        loop {
            if owns() {
                match crate::memory::tick(&self.vault, 45, 4).await {
                    Ok(0) => {}
                    Ok(n) => self.emit("memory", json!({"summaries": n})),
                    Err(e) => tracing::warn!("memory upkeep: {e}"),
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }

    // ───────── product rooms ─────────

    /// Rename a room. Every thread gets a notice, every member gets one in mux_updates (and the
    /// Claude Code hook), and members that keep memory between runs (webhook bots) are woken with it.
    pub fn rename_room(self: &Arc<Self>, room: &str, title: &str, by: &str) -> Result<String> {
        let (old, new) = self.vault.rename_room(room, title)?;
        if old == new {
            return Ok(new);
        }
        let who = if by == "user" { "You".to_string() } else { self.vault.agent_name(by) };
        let text = format!("Room renamed: \"{old}\" is now \"{new}\" (by {who}). Use the new name when you refer to it.");
        self.announce(room, &text, by)?;
        Ok(new)
    }

    /// File the caller's folder under a product room, by name. Creates the room if it doesn't exist.
    pub fn use_room(self: &Arc<Self>, ctx: &CallerCtx, name: &str) -> Result<(String, bool)> {
        let root = context::workspace_root(Path::new(&ctx.cwd));
        if root == config::home() {
            bail!("this agent has no workspace folder to file under a room");
        }
        let (room, created) = match self.vault.find_room(name)? {
            Some(r) => (r, false),
            None => (self.vault.create_room(name)?, true),
        };
        self.vault.assign_folder(&root.to_string_lossy(), &room)?;
        if self.vault.is_agent(&ctx.agent) {
            self.vault.add_member(&room, &ctx.agent)?;
        }
        self.emit("room", json!({"id": room}));
        Ok((room, created))
    }

    /// Merge room `from` into `into`, then tell everyone involved.
    pub fn merge_rooms(self: &Arc<Self>, from: &str, into: &str, by: &str) -> Result<()> {
        let name = |id: &str| self.vault.room(id).ok().flatten().and_then(|r| r["title"].as_str().map(String::from)).unwrap_or_else(|| id.to_string());
        let (a, b) = (name(from), name(into));
        self.vault.merge_room(from, into)?;
        self.announce(into, &format!("Room \"{a}\" was merged into \"{b}\". Its threads and folders are here now."), by)?;
        self.emit("room", json!({"id": into}));
        Ok(())
    }

    /// Delete a room for good (the user's call, from the app). Everyone who took part is told;
    /// attachment files AgentMux stored for it are removed from its media folder.
    pub fn delete_room(self: &Arc<Self>, room: &str, by: &str) -> Result<()> {
        let title = self.vault.room(room)?.and_then(|r| r["title"].as_str().map(String::from)).unwrap_or_else(|| room.to_string());
        let members = self.vault.room_members(room).unwrap_or_default();
        let files = self.vault.delete_room(room)?;
        let media = crate::media::dir(&self.vault);
        for f in files {
            let p = std::path::PathBuf::from(&f);
            if p.starts_with(&media) {
                let _ = std::fs::remove_file(p);
            }
        }
        let who = if by == "user" { "the user".to_string() } else { self.vault.agent_name(by) };
        for m in members.iter().filter(|m| m.as_str() != by) {
            let _ = self.vault.add_notice(m, "", &format!("Room \"{title}\" was deleted by {who}, with its threads. Don't refer to it again."));
        }
        self.emit("room", json!({"id": room, "deleted": true}));
        Ok(())
    }

    /// Rename an agent (its display name; its @id, and everything addressed to it, stays). Every
    /// thread it took part in gets a note, every agent that shared one with it gets a notice, and
    /// the agent itself is told (woken, if it's a bot with a webhook) so it uses its new name.
    pub fn rename_agent(self: &Arc<Self>, id: &str, name: &str, by: &str) -> Result<String> {
        if !self.vault.is_agent(id) {
            bail!("unknown agent '{id}'");
        }
        let old = self.vault.agent_name(id);
        self.vault.set_profile(id, name, None)?;
        let new = self.vault.agent_name(id);
        if old == new {
            return Ok(new);
        }
        let who = if by == "user" { "You".to_string() } else { self.vault.agent_name(by) };
        let text = format!("Agent renamed: \"{old}\" is now \"{new}\" (by {who}). Its address @{id} is unchanged.");
        let mut told: Vec<String> = vec![];
        for (sess, room) in self.vault.agent_sessions(id)? {
            let msg = self.vault.add_message(&sess, &room, "agentmux", Some("all"), "system", &text, None, None)?;
            self.emit("message", msg);
            for m in self.vault.session_members(&sess)? {
                if m != by && !told.contains(&m) {
                    told.push(m);
                }
            }
        }
        if !told.iter().any(|m| m == id) && by != id {
            told.push(id.to_string());
        }
        for m in &told {
            let note = if m == id { format!("You were renamed: you are now \"{new}\" (address @{id}). Use this name from now on.") } else { text.clone() };
            self.vault.add_notice(m, "", &note)?;
        }
        self.emit("provider_connected", json!({"id": id}));
        // A bot with a webhook remembers between runs: tell it now.
        let webhook = self.vault.providers()?.into_iter().find(|p| p["id"] == id).is_some_and(|p| p["wakeKind"] == "webhook");
        if webhook && by != id {
            if let (Ok(rt), Ok((url, key))) = (tokio::runtime::Handle::try_current(), webhook::load(&self.vault, id)) {
                let ctx = format!("agentmux_notice\nFor: @{id}\n\nYou were renamed in AgentMux: you are now \"{new}\" (address @{id} unchanged). Use this name from now on; no mux_reply is needed.\n");
                rt.spawn(async move {
                    let _ = webhook::post(&url, &key, &ctx).await;
                });
            }
        }
        Ok(new)
    }

    /// Post `text` into every thread of a room, notify each member except `by`, and wake the
    /// members that remember between runs (webhook bots; `rooms.wake_on_change`, on by default).
    /// Headless CLI runs start fresh each time, so they get it on their next turn instead.
    fn announce(self: &Arc<Self>, room: &str, text: &str, by: &str) -> Result<()> {
        for sess in self.vault.session_ids(room)? {
            let msg = self.vault.add_message(&sess, room, "agentmux", Some("all"), "system", text, None, None)?;
            self.emit("message", msg);
        }
        let members: Vec<String> = self.vault.room_members(room)?.into_iter().filter(|m| m != by && m != "user").collect();
        for m in &members {
            self.vault.add_notice(m, room, text)?;
        }
        self.emit("room", json!({"id": room}));
        let wake = self.vault.setting("rooms.wake_on_change").ok().flatten().as_deref() != Some("0");
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return Ok(()) };
        if !wake {
            return Ok(());
        }
        for m in members {
            let kind = self.vault.providers()?.into_iter().find(|p| p["id"] == m.as_str()).and_then(|p| p["wakeKind"].as_str().map(String::from));
            if kind.as_deref() != Some("webhook") {
                continue;
            }
            let Ok((url, key)) = webhook::load(&self.vault, &m) else { continue };
            let ctx = format!("agentmux_notice\nFor: @{m}\n\n{text}\n\nThis is a notice, not a question: no mux_reply is needed.\n");
            rt.spawn(async move {
                if let Err(e) = webhook::post(&url, &key, &ctx).await {
                    tracing::warn!("could not notify @{m}: {e}");
                }
            });
        }
        Ok(())
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
            if let Some(id) = self.vault.find_room(r)? {
                let sess = self.vault.latest_session(&id)?.ok_or_else(|| anyhow!("room {r} has no session"))?;
                return Ok((id, sess));
            }
            bail!("unknown room '{r}' (see mux_list_rooms)");
        }
        let root = context::workspace_root(Path::new(&ctx.cwd));
        // Not a project folder (home, an app's scratch folder): the product this agent was last
        // talking about, else the Inbox. Never a room named after the folder.
        if context::is_scratch_dir(&root) {
            let room = match self.vault.setting(&format!("agent.{}.room", ctx.agent))?.filter(|r| self.vault.room(r).ok().flatten().is_some()) {
                Some(r) => r,
                None => match self.vault.find_room("Inbox")? {
                    Some(r) => r,
                    None => self.vault.create_room("Inbox")?,
                },
            };
            let sess = match self.vault.latest_session(&room)? {
                Some(s) => s,
                None => self.vault.new_session(&room, "")?,
            };
            if self.vault.is_agent(&ctx.agent) {
                self.vault.add_member(&room, &ctx.agent)?;
            }
            return Ok((room, sess));
        }
        let (branch, _) = context::capture_git(&root);
        let (room, sess) = self.vault.ensure_room(&root.to_string_lossy(), &branch)?;
        if self.vault.is_agent(&ctx.agent) {
            self.vault.add_member(&room, &ctx.agent)?;
        }
        Ok((room, sess))
    }

    /// Does this caller work in a real project folder? (False for home and scratch folders.)
    pub fn in_project(&self, ctx: &CallerCtx) -> bool {
        let root = context::workspace_root(Path::new(&ctx.cwd));
        !context::is_scratch_dir(&root)
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

    /// Which session a new question belongs to. A session is one topic of conversation, reused
    /// until someone asks for a new one:
    ///  1. `new_session`: always a fresh session (named after the topic, if given);
    ///  2. a `topic`: the room's session on that topic, or a new one named after it;
    ///  3. otherwise the latest session if it is still empty (the UI's "New session");
    ///  4. otherwise the session where these two agents last talked, however long ago;
    ///  5. otherwise a new session for this pair.
    fn pick_session(&self, room: &str, latest: &str, from: &str, to: &str, opts: &AskOpts) -> Result<String> {
        let who = |id: &str| if id == "user" { "You".to_string() } else { self.vault.agent_name(id) };
        let topic = opts.topic.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if opts.new_session {
            return self.vault.new_session(room, topic.unwrap_or(&format!("{} ↔ {}", who(from), who(to))));
        }
        let latest_empty = self.vault.session_message_count(latest)? == 0;
        if let Some(t) = topic {
            if let Some(s) = self.vault.session_for_topic(room, t)? {
                return Ok(s);
            }
            if latest_empty {
                self.vault.set_session_topic_if_unnamed(latest, t)?;
                return Ok(latest.to_string());
            }
            return self.vault.new_session(room, t);
        }
        if latest_empty {
            return Ok(latest.to_string());
        }
        if let Some(s) = self.vault.last_pair_session(room, from, to)? {
            return Ok(s);
        }
        self.vault.new_session(room, &format!("{} ↔ {}", who(from), who(to)))
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
        self.start_ticket_with(ctx, room_hint, session_hint, to, question, then, &AskOpts::default())
    }

    /// `start_ticket_in` with a topic / new-session choice (see `pick_session`).
    #[allow(clippy::too_many_arguments)]
    pub fn start_ticket_with(
        self: &Arc<Self>,
        ctx: &CallerCtx,
        room_hint: Option<&str>,
        session_hint: Option<&str>,
        to: &str,
        question: &str,
        then: Option<String>,
        opts: &AskOpts,
    ) -> Result<String> {
        // The wake runs as a background task, so this must be called inside the async runtime. Checking
        // first means a caller outside it gets an error instead of a ticket that is never woken.
        let rt = tokio::runtime::Handle::try_current().map_err(|_| anyhow!("internal error: asking an agent must run inside the async runtime"))?;
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
        // The caller's chat, when its client tells us which one it is (Claude Code via its hook).
        let root = context::workspace_root(Path::new(&ctx.cwd));
        let thread = self.vault.active_thread(&ctx.agent, &root.to_string_lossy())?;
        let bound = thread.as_ref().and_then(|t| t.1.clone()).filter(|s| self.vault.session_room(s).ok().flatten().as_deref() == Some(room.as_str()));
        let sess = match (session_hint.filter(|s| !s.is_empty()), session_room) {
            (Some(s), Some(r)) if r == room => s.to_string(),
            (Some(s), _) => bail!("session '{s}' is not in room '{room}'"),
            // This chat already has a thread here: keep talking in it unless a topic says otherwise.
            _ if opts.topic.is_none() && !opts.new_session && bound.is_some() => bound.clone().unwrap(),
            _ => self.pick_session(&room, &latest, &ctx.agent, to, opts)?,
        };
        if let Some((key, _)) = &thread {
            self.vault.bind_thread(&ctx.agent, key, &sess)?;
        }
        self.vault.add_member(&room, to)?;

        // Snapshot the asker's live git state so the peer sees what the caller sees. A product room
        // can span several folders, so it's the caller's folder, not the room's first one.
        let caller_ws = context::workspace_root(Path::new(&ctx.cwd));
        if caller_ws != config::home() {
            let (_, diff) = context::capture_git(&caller_ws);
            if !diff.is_empty() {
                self.vault.set_context(&room, "diff", &diff)?;
            }
        }

        let ticket = self.vault.create_ticket(&sess, &room, &ctx.agent, to, question, hop, then.as_deref())?;
        // The product this agent is on now (used when it asks from a non-project folder).
        if self.vault.is_agent(&ctx.agent) {
            self.vault.set_setting(&format!("agent.{}.room", ctx.agent), &room)?;
        }
        let msg = self.vault.add_message(&sess, &room, &ctx.agent, Some(to), "a2a_request", question, None, Some(&ticket))?;
        if let Some(mid) = msg["id"].as_str() {
            crate::media::link(&self.vault, &opts.attachments, mid)?;
        }
        self.emit("message", msg);
        self.notify_thread(&sess, &room, &ctx.agent, Some(to), question)?;
        self.emit("ticket", json!({"id": ticket, "status": "pending", "from": ctx.agent, "to": to, "room": room}));
        let _ = self.vault.export_markdown(&room);

        let core = self.clone();
        let t = ticket.clone();
        rt.spawn(async move { core.run_ticket(t).await });
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
        self.finish_with(ticket_id, status, text, code, &[])
    }

    /// `finish` with attachments (already stored) linked to the answer.
    pub fn finish_with(self: &Arc<Self>, ticket_id: &str, status: &str, text: &str, code: Option<&str>, attachments: &[String]) -> Result<bool> {
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
        if let Some(mid) = msg["id"].as_str() {
            crate::media::link(&self.vault, attachments, mid)?;
        }
        let _ = self.vault.touch_provider(&to);
        self.emit("message", msg);
        if status == "answered" {
            self.notify_thread(&sess, &room, &to, Some(&from), text)?;
        }
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

    /// Close questions that were never going to be answered: the recipient is woken by AgentMux (CLI or
    /// webhook), the wake window has long passed, and nothing finished the ticket (the app was restarted
    /// or the wake never started). Without this they stay open and block the next question to that agent.
    pub fn sweep_stale(self: &Arc<Self>) -> usize {
        self.sweep_stale_after(60)
    }

    pub fn sweep_stale_after(self: &Arc<Self>, grace_secs: i64) -> usize {
        let kinds: std::collections::HashMap<String, String> = self
            .vault
            .providers()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| Some((p["id"].as_str()?.to_string(), p["wakeKind"].as_str()?.to_string())))
            .collect();
        let now = chrono::Utc::now();
        let mut n = 0;
        for t in self.vault.open_tickets().unwrap_or_default() {
            let (Some(id), Some(to), Some(made)) = (t["id"].as_str(), t["to"].as_str(), t["createdAt"].as_str()) else { continue };
            let window = match kinds.get(to).map(String::as_str) {
                Some("cli") => config::wake_timeout_secs(),
                Some("webhook") => config::webhook_timeout_secs(),
                _ => continue, // inbox-only agents answer whenever they next look
            };
            let Ok(made) = chrono::DateTime::parse_from_rfc3339(made) else { continue };
            if (now - made.with_timezone(&chrono::Utc)).num_seconds() > window as i64 + grace_secs {
                let msg = "No answer arrived and the wake isn't running (AgentMux may have restarted). Ask again.";
                if self.finish(id, "expired", msg, None).unwrap_or(false) {
                    n += 1;
                }
            }
        }
        n
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
    let atts = crate::media::for_ticket_request(vault, t["id"].as_str().unwrap_or("")).unwrap_or_default();
    if !atts.is_empty() {
        p.push_str("\nAttached files (open them from disk):\n");
        for a in atts {
            p.push_str(&format!("- {} ({}){}\n", a["path"].as_str().unwrap_or(""), a["mime"].as_str().unwrap_or(""),
                a["caption"].as_str().map(|c| format!(": {c}")).unwrap_or_default()));
        }
    }
    p.push_str("\nAnswer directly and concisely. Your final message is delivered back to the asker as the reply.");
    if !allow_edits {
        p.push_str(" Do not modify any files.");
    }
    p
}
