use crate::config;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS providers(
  id TEXT PRIMARY KEY, name TEXT, wake_kind TEXT,
  connected_at TEXT, last_seen TEXT, calls INTEGER DEFAULT 0);
CREATE TABLE IF NOT EXISTS rooms(
  id TEXT PRIMARY KEY, title TEXT, kind TEXT, workspace_path TEXT,
  branch TEXT, allow_edits INTEGER DEFAULT 0, updated_at TEXT);
CREATE TABLE IF NOT EXISTS room_members(
  room_id TEXT, provider_id TEXT, PRIMARY KEY(room_id, provider_id));
CREATE TABLE IF NOT EXISTS sessions(
  id TEXT PRIMARY KEY, room_id TEXT, title TEXT, status TEXT, started_at TEXT);
CREATE TABLE IF NOT EXISTS messages(
  id TEXT PRIMARY KEY, session_id TEXT, room_id TEXT, sender TEXT, recipient TEXT,
  kind TEXT, text TEXT, code TEXT, ticket_id TEXT, created_at TEXT);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text, content='messages', content_rowid='rowid');
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TABLE IF NOT EXISTS tickets(
  id TEXT PRIMARY KEY, session_id TEXT, room_id TEXT, sender TEXT, recipient TEXT,
  question TEXT, status TEXT, hop INTEGER, then_to TEXT, answer TEXT,
  created_at TEXT, answered_at TEXT);
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS context(
  room_id TEXT, kind TEXT, content TEXT, captured_at TEXT, PRIMARY KEY(room_id, kind));
"#;

/// Settings that travel with a sync. Webhook URLs, tokens and tunnel ids are deliberately absent:
/// they belong to one machine's Keychain and Cloudflare login.
pub const SYNC_SETTINGS: &[&str] = &[
    "tunnel.provider",
    "tunnel.cf_hostname",
    "tunnel.cf_zone",
    "tunnel.cf_mode",
    "tunnel.ngrok_domain",
    "tunnel.autostart",
    "updates.auto",
];

pub struct Vault {
    conn: Mutex<Connection>,
    threads_dir: PathBuf,
}

pub fn now() -> String {
    Utc::now().to_rfc3339()
}

pub fn new_id(prefix: &str) -> String {
    format!("{}-{}", prefix, &uuid::Uuid::new_v4().simple().to_string()[..12])
}

fn secure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Atomic write: tmp file (0600) + fsync + rename.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Vault {
    pub fn open() -> Result<Self> {
        Self::open_at(&config::db_path(), &config::threads_dir())
    }

    pub fn open_at(db: &Path, threads_dir: &Path) -> Result<Self> {
        if let Some(parent) = db.parent() {
            secure_dir(parent)?;
        }
        secure_dir(threads_dir)?;
        let conn = Connection::open(db).context("open vault.db")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(db, std::fs::Permissions::from_mode(0o600));
        }
        let v = Vault { conn: Mutex::new(conn), threads_dir: threads_dir.to_path_buf() };
        v.seed_providers()?;
        Ok(v)
    }

    // ---------- providers ----------

    /// Make sure every registered provider (built-in or manifest) has a row. Cheap; safe to repeat.
    pub fn seed_providers(&self) -> Result<()> {
        for p in crate::providers::all() {
            let i = p.info();
            self.conn.lock().unwrap().execute(
                "INSERT OR IGNORE INTO providers(id, name, wake_kind, calls, family) VALUES(?1, ?2, ?3, 0, ?4)",
                params![i.id, i.name, default_wake_kind(&i.id), family_of(&i.id)],
            )?;
        }
        Ok(())
    }

    /// AgentMux was removed from the agent's config: it is no longer connected until its next handshake.
    pub fn clear_connection(&self, id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute("UPDATE providers SET connected_at = NULL WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Mark a provider as having completed a real MCP handshake.
    pub fn mark_connected(&self, id: &str) -> Result<()> {
        let t = now();
        self.conn.lock().unwrap().execute(
            "UPDATE providers SET connected_at = COALESCE(connected_at, ?2), last_seen = ?2 WHERE id = ?1",
            params![id, t],
        )?;
        Ok(())
    }

    pub fn touch_provider(&self, id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE providers SET last_seen = ?2, calls = calls + 1 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    pub fn set_wake_kind(&self, id: &str, kind: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("UPDATE providers SET wake_kind = ?2 WHERE id = ?1", params![id, kind])?;
        Ok(())
    }

    pub fn providers(&self) -> Result<Vec<Value>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id, name, wake_kind, connected_at, last_seen, calls, description, family, registered FROM providers ORDER BY rowid",
        )?;
        let rows = st.query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                "wakeKind": r.get::<_, Option<String>>(2)?,
                "connectedAt": r.get::<_, Option<String>>(3)?,
                "lastSeen": r.get::<_, Option<String>>(4)?,
                "calls": r.get::<_, i64>(5)?,
                "description": r.get::<_, Option<String>>(6)?,
                "family": r.get::<_, Option<String>>(7)?,
                "registered": r.get::<_, Option<i64>>(8)?.unwrap_or(0) != 0,
            }))
        })?;
        let mut out: Vec<Value> = rows.collect::<std::result::Result<_, _>>()?;
        drop(st);
        drop(c);
        for p in out.iter_mut() {
            let id = p["id"].as_str().unwrap_or("").to_string();
            p["canAsk"] = json!(self.can_ask(&id));
        }
        Ok(out)
    }

    /// May this Grok bot start conversations with other agents over the tunnel? On unless switched off.
    pub fn can_ask(&self, id: &str) -> bool {
        config::is_grok_family(id) && self.setting(&format!("bot.{id}.can_ask")).ok().flatten().as_deref() != Some("0")
    }

    pub fn set_can_ask(&self, id: &str, on: bool) -> Result<()> {
        if !config::is_grok_family(id) {
            anyhow::bail!("only Grok bots use this setting");
        }
        if !self.is_agent(id) {
            anyhow::bail!("unknown bot '{id}'");
        }
        let key = format!("bot.{id}.can_ask");
        self.set_setting(&key, if on { "1" } else { "0" })
    }

    /// A fixed agent or a bot that exists in `providers`.
    pub fn is_agent(&self, id: &str) -> bool {
        config::is_known_agent(id)
            || self
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT 1 FROM providers WHERE id = ?1", params![id], |_| Ok(()))
                .optional()
                .ok()
                .flatten()
                .is_some()
    }

    pub fn agent_name(&self, id: &str) -> String {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT name FROM providers WHERE id = ?1", params![id], |r| r.get::<_, String>(0))
            .optional()
            .ok()
            .flatten()
            .unwrap_or_else(|| config::agent_name(id))
    }

    /// Create another Grok bot contact; returns its id (`grok:<slug>`).
    pub fn add_bot(&self, name: &str) -> Result<String> {
        let name = name.trim();
        let base = config::slug(name);
        if base.is_empty() {
            anyhow::bail!("give the bot a name (letters or numbers)");
        }
        let c = self.conn.lock().unwrap();
        let mut id = format!("grok:{base}");
        let mut n = 2;
        while c.query_row("SELECT 1 FROM providers WHERE id = ?1", params![id], |_| Ok(())).optional()?.is_some() {
            id = format!("grok:{base}-{n}");
            n += 1;
        }
        c.execute(
            "INSERT INTO providers(id, name, wake_kind, calls, family, registered) VALUES(?1, ?2, 'inbox', 0, 'grok', 1)",
            params![id, name],
        )?;
        Ok(id)
    }

    /// Delete a bot contact. The fixed agents (including the original `grok`) cannot be removed.
    pub fn remove_bot(&self, id: &str) -> Result<()> {
        if !id.starts_with("grok:") {
            anyhow::bail!("only added Grok bots can be removed");
        }
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        c.execute("DELETE FROM room_members WHERE provider_id = ?1", params![id])?;
        c.execute("DELETE FROM settings WHERE key = ?1", params![format!("bot.{id}.can_ask")])?;
        Ok(())
    }

    /// The agent describes itself (or the user renames it).
    pub fn set_profile(&self, id: &str, name: &str, description: Option<&str>) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            anyhow::bail!("name must be 1-60 characters");
        }
        let desc: Option<String> = description.map(|d| d.trim().chars().take(240).collect()).filter(|d: &String| !d.is_empty());
        self.conn.lock().unwrap().execute(
            "UPDATE providers SET name = ?2, description = COALESCE(?3, description), registered = 1 WHERE id = ?1",
            params![id, name, desc],
        )?;
        Ok(())
    }

    // ---------- rooms / sessions ----------

    /// Find or create the room for a workspace; returns (room_id, session_id).
    pub fn ensure_room(&self, workspace: &str, branch: &str) -> Result<(String, String)> {
        let slug = slugify(workspace);
        // Cloud bots have no folder; their questions use the AgentMux home as a neutral room.
        let title = if Path::new(workspace) == config::home() {
            "Cloud bots".to_string()
        } else {
            Path::new(workspace).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| workspace.to_string())
        };
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT OR IGNORE INTO rooms(id, title, kind, workspace_path, branch, updated_at) VALUES(?1, ?2, 'group', ?3, ?4, ?5)",
            params![slug, title, workspace, branch, now()],
        )?;
        c.execute("UPDATE rooms SET branch = ?2 WHERE id = ?1 AND ?2 != ''", params![slug, branch])?;
        let sess: Option<String> = c
            .query_row(
                "SELECT id FROM sessions WHERE room_id = ?1 ORDER BY started_at DESC LIMIT 1",
                params![slug],
                |r| r.get(0),
            )
            .optional()?;
        let sid = match sess {
            Some(s) => s,
            None => {
                let sid = new_id("sess");
                c.execute(
                    "INSERT INTO sessions(id, room_id, title, status, started_at) VALUES(?1, ?2, 'Session #1', 'Active', ?3)",
                    params![sid, slug, now()],
                )?;
                sid
            }
        };
        Ok((slug, sid))
    }

    pub fn latest_session(&self, room: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id FROM sessions WHERE room_id = ?1 ORDER BY started_at DESC LIMIT 1",
                params![room],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn add_member(&self, room: &str, agent: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO room_members(room_id, provider_id) VALUES(?1, ?2)",
            params![room, agent],
        )?;
        Ok(())
    }

    /// A new session. A non-empty `title` names it (and, as a topic, routes later questions on the
    /// same topic to it); otherwise it is "Session #N".
    pub fn new_session(&self, room: &str, title: &str) -> Result<String> {
        let c = self.conn.lock().unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM sessions WHERE room_id = ?1", params![room], |r| r.get(0))?;
        let sid = new_id("sess");
        let (title, topic) = match clean_title(title) {
            Some(t) => (t.clone(), Some(topic_key(&t))),
            None => (format!("Session #{}", n + 1), None),
        };
        c.execute(
            "INSERT INTO sessions(id, room_id, title, status, started_at, topic) VALUES(?1, ?2, ?3, 'Active', ?4, ?5)",
            params![sid, room, title, now(), topic],
        )?;
        Ok(sid)
    }

    /// Rename a session. The new name is also its topic, so later questions using it land here.
    pub fn rename_session(&self, session: &str, title: &str) -> Result<()> {
        let t = clean_title(title).ok_or_else(|| anyhow::anyhow!("a session needs a name"))?;
        let n = self.conn.lock().unwrap().execute(
            "UPDATE sessions SET title = ?2, topic = ?3 WHERE id = ?1",
            params![session, t, topic_key(&t)],
        )?;
        if n == 0 {
            anyhow::bail!("unknown session '{session}'");
        }
        Ok(())
    }

    pub fn rename_room(&self, room: &str, title: &str) -> Result<()> {
        let t = clean_title(title).ok_or_else(|| anyhow::anyhow!("a room needs a name"))?;
        let n = self.conn.lock().unwrap().execute("UPDATE rooms SET title = ?2 WHERE id = ?1", params![room, t])?;
        if n == 0 {
            anyhow::bail!("unknown room '{room}'");
        }
        Ok(())
    }

    /// A room by id or, case-insensitively, by its name.
    pub fn find_room(&self, id_or_name: &str) -> Result<Option<String>> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row(
            "SELECT id FROM rooms WHERE id = ?1 OR lower(title) = lower(?1) ORDER BY id = ?1 DESC, updated_at DESC LIMIT 1",
            params![id_or_name.trim()],
            |r| r.get(0),
        )
        .optional()?)
    }

    /// The room's session whose topic or name matches `topic`, most recently active first.
    pub fn session_for_topic(&self, room: &str, topic: &str) -> Result<Option<String>> {
        let want = topic_key(topic);
        if want.is_empty() {
            return Ok(None);
        }
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT s.id, s.title, s.topic FROM sessions s WHERE s.room_id = ?1
             ORDER BY COALESCE((SELECT MAX(created_at) FROM messages m WHERE m.session_id = s.id), s.started_at) DESC",
        )?;
        let rows: Vec<(String, String, Option<String>)> =
            st.query_map(params![room], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
        Ok(rows.into_iter().find(|(_, title, t)| topics_match(&want, t.as_deref().unwrap_or(&topic_key(title)))).map(|(id, ..)| id))
    }

    /// Give an unnamed session a topic (the first question that names one).
    pub fn set_session_topic_if_unnamed(&self, session: &str, topic: &str) -> Result<()> {
        if let Some(t) = clean_title(topic) {
            self.conn.lock().unwrap().execute(
                "UPDATE sessions SET title = ?2, topic = ?3 WHERE id = ?1 AND topic IS NULL AND title LIKE 'Session #%'",
                params![session, t, topic_key(&t)],
            )?;
        }
        Ok(())
    }

    /// The session in this room where `a` and `b` last talked, however long ago.
    pub fn last_pair_session(&self, room: &str, a: &str, b: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT session_id FROM tickets WHERE room_id = ?1
                   AND ((sender = ?2 AND recipient = ?3) OR (sender = ?3 AND recipient = ?2))
                 ORDER BY created_at DESC LIMIT 1",
                params![room, a, b],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The room's sessions for an agent choosing where to talk: name, activity and who took part.
    pub fn session_list(&self, room: &str) -> Result<Vec<Value>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT s.id, s.title, s.started_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id),
                    (SELECT MAX(created_at) FROM messages m WHERE m.session_id = s.id),
                    (SELECT group_concat(DISTINCT sender) FROM messages m WHERE m.session_id = s.id)
             FROM sessions s WHERE s.room_id = ?1",
        )?;
        let mut rows: Vec<Value> = st
            .query_map(params![room], |r| {
                let who: Option<String> = r.get(5)?;
                Ok(json!({"session": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                          "messages": r.get::<_, i64>(3)?, "lastActivity": r.get::<_, Option<String>>(4)?.unwrap_or(r.get::<_, String>(2)?),
                          "participants": who.map(|w| w.split(',').map(String::from).collect::<Vec<_>>()).unwrap_or_default()}))
            })?
            .collect::<std::result::Result<_, _>>()?;
        rows.sort_by(|a, b| b["lastActivity"].as_str().cmp(&a["lastActivity"].as_str()));
        Ok(rows)
    }

    pub fn session_room(&self, session: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT room_id FROM sessions WHERE id = ?1", params![session], |r| r.get(0))
            .optional()?)
    }

    pub fn session_message_count(&self, session: &str) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM messages WHERE session_id = ?1", params![session], |r| r.get(0))?)
    }

    pub fn room(&self, room: &str) -> Result<Option<Value>> {
        let c = self.conn.lock().unwrap();
        let base = c
            .query_row(
                "SELECT id, title, kind, workspace_path, branch, allow_edits, updated_at FROM rooms WHERE id = ?1",
                params![room],
                |r| {
                    Ok(json!({
                        "id": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?,
                        "kind": r.get::<_, String>(2)?, "workspacePath": r.get::<_, String>(3)?,
                        "branch": r.get::<_, Option<String>>(4)?, "allowEdits": r.get::<_, i64>(5)? != 0,
                        "updatedAt": r.get::<_, String>(6)?,
                    }))
                },
            )
            .optional()?;
        let Some(mut base) = base else { return Ok(None) };
        let mut st = c.prepare("SELECT provider_id FROM room_members WHERE room_id = ?1 ORDER BY rowid")?;
        let members: Vec<String> = st.query_map(params![room], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        base["participants"] = json!(members);
        let mut st = c.prepare("SELECT id, title, status, started_at, topic FROM sessions WHERE room_id = ?1 ORDER BY started_at DESC")?;
        let sessions: Vec<Value> = st
            .query_map(params![room], |r| {
                Ok(json!({"id": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?,
                          "status": r.get::<_, String>(2)?, "startedAt": r.get::<_, String>(3)?,
                          "topic": r.get::<_, Option<String>>(4)?}))
            })?
            .collect::<std::result::Result<_, _>>()?;
        base["sessions"] = json!(sessions);
        let mut ctx = serde_json::Map::new();
        let mut st = c.prepare("SELECT kind, content FROM context WHERE room_id = ?1")?;
        for row in st.query_map(params![room], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (k, v) = row?;
            ctx.insert(k, json!(v));
        }
        base["context"] = Value::Object(ctx);
        Ok(Some(base))
    }

    pub fn room_ids(&self) -> Result<Vec<String>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare("SELECT id FROM rooms ORDER BY updated_at DESC")?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?)
    }

    pub fn set_allow_edits(&self, room: &str, allow: bool) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("UPDATE rooms SET allow_edits = ?2 WHERE id = ?1", params![room, allow as i64])?;
        Ok(())
    }

    // ---------- messages ----------

    #[allow(clippy::too_many_arguments)]
    pub fn add_message(
        &self,
        session: &str,
        room: &str,
        sender: &str,
        recipient: Option<&str>,
        kind: &str,
        text: &str,
        code: Option<&str>,
        ticket: Option<&str>,
    ) -> Result<Value> {
        let id = new_id("m");
        let ts = now();
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO messages(id, session_id, room_id, sender, recipient, kind, text, code, ticket_id, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![id, session, room, sender, recipient, kind, text, code, ticket, ts],
        )?;
        c.execute("UPDATE rooms SET updated_at = ?2 WHERE id = ?1", params![room, ts])?;
        Ok(json!({"id": id, "sessionId": session, "roomId": room, "sender": sender, "recipient": recipient,
                  "type": kind, "text": text, "codeBlock": code, "ticketId": ticket, "createdAt": ts}))
    }

    pub fn messages(&self, session: &str) -> Result<Vec<Value>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id, sender, recipient, kind, text, code, ticket_id, created_at FROM messages WHERE session_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = st.query_map(params![session], |r| {
            Ok(json!({"id": r.get::<_, String>(0)?, "sender": r.get::<_, String>(1)?,
                      "recipient": r.get::<_, Option<String>>(2)?, "type": r.get::<_, String>(3)?,
                      "text": r.get::<_, String>(4)?, "codeBlock": r.get::<_, Option<String>>(5)?,
                      "ticketId": r.get::<_, Option<String>>(6)?, "createdAt": r.get::<_, String>(7)?}))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn search(&self, query: &str) -> Result<Vec<Value>> {
        let c = self.conn.lock().unwrap();
        // Quote as a phrase so user input can't be parsed as FTS syntax.
        let q = format!("\"{}\"", query.replace('"', "\"\""));
        let mut st = c.prepare(
            "SELECT m.room_id, m.sender, m.text, m.created_at FROM messages_fts f
             JOIN messages m ON m.rowid = f.rowid WHERE messages_fts MATCH ?1 ORDER BY rank LIMIT 50",
        )?;
        let rows = st.query_map(params![q], |r| {
            Ok(json!({"roomId": r.get::<_, String>(0)?, "sender": r.get::<_, String>(1)?,
                      "text": r.get::<_, String>(2)?, "createdAt": r.get::<_, String>(3)?}))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // ---------- tickets ----------

    #[allow(clippy::too_many_arguments)]
    pub fn create_ticket(
        &self,
        session: &str,
        room: &str,
        sender: &str,
        recipient: &str,
        question: &str,
        hop: i64,
        then_to: Option<&str>,
    ) -> Result<String> {
        let id = new_id("T");
        self.conn.lock().unwrap().execute(
            "INSERT INTO tickets(id, session_id, room_id, sender, recipient, question, status, hop, then_to, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,'pending',?7,?8,?9)",
            params![id, session, room, sender, recipient, question, hop, then_to, now()],
        )?;
        Ok(id)
    }

    pub fn open_ticket_between(&self, from: &str, to: &str) -> Result<bool> {
        let n: i64 = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM tickets WHERE sender = ?1 AND recipient = ?2 AND status IN ('pending','running')",
            params![from, to],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Tickets still waiting for an answer.
    pub fn open_tickets(&self) -> Result<Vec<Value>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare("SELECT id, sender, recipient, status, created_at FROM tickets WHERE status IN ('pending','running')")?;
        let rows = st.query_map([], |r| {
            Ok(json!({"id": r.get::<_, String>(0)?, "from": r.get::<_, String>(1)?, "to": r.get::<_, String>(2)?, "status": r.get::<_, String>(3)?, "createdAt": r.get::<_, String>(4)?}))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn ticket(&self, id: &str) -> Result<Option<Value>> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row(
            "SELECT id, session_id, room_id, sender, recipient, question, status, hop, then_to, answer, created_at, answered_at
             FROM tickets WHERE id = ?1",
            params![id],
            |r| {
                Ok(json!({"id": r.get::<_, String>(0)?, "sessionId": r.get::<_, String>(1)?,
                    "roomId": r.get::<_, String>(2)?, "from": r.get::<_, String>(3)?,
                    "to": r.get::<_, String>(4)?, "question": r.get::<_, String>(5)?,
                    "status": r.get::<_, String>(6)?, "hop": r.get::<_, i64>(7)?,
                    "then": r.get::<_, Option<String>>(8)?, "answer": r.get::<_, Option<String>>(9)?,
                    "createdAt": r.get::<_, String>(10)?, "answeredAt": r.get::<_, Option<String>>(11)?}))
            },
        )
        .optional()?)
    }

    pub fn set_ticket_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("UPDATE tickets SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(())
    }

    /// Returns false if the ticket was already closed (first answer wins). A ticket that only
    /// timed out (`expired`) still accepts a late answer.
    pub fn answer_ticket(&self, id: &str, status: &str, answer: &str) -> Result<bool> {
        let n = self.conn.lock().unwrap().execute(
            "UPDATE tickets SET status = ?2, answer = ?3, answered_at = ?4
             WHERE id = ?1 AND (status IN ('pending','running') OR (status = 'expired' AND ?2 = 'answered'))",
            params![id, status, answer, now()],
        )?;
        Ok(n > 0)
    }

    pub fn inbox(&self, agent: &str) -> Result<Vec<Value>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id, sender, question, room_id, created_at FROM tickets WHERE recipient = ?1 AND status IN ('pending','running') ORDER BY created_at",
        )?;
        let rows = st.query_map(params![agent], |r| {
            Ok(json!({"ticket": r.get::<_, String>(0)?, "from": r.get::<_, String>(1)?,
                      "question": r.get::<_, String>(2)?, "room": r.get::<_, String>(3)?,
                      "createdAt": r.get::<_, String>(4)?}))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// What's new for `agent` since its read cursor: answers to its questions (late ones too) and
    /// questions addressed to it. Oldest first, at most 20. With `mark_read`, the cursor advances.
    /// The first read starts one day back so a new install doesn't replay all history.
    pub fn updates_for(&self, agent: &str, mark_read: bool) -> Result<Vec<Value>> {
        let key = format!("updates.{agent}.seen");
        let since = self.setting(&key)?.unwrap_or_else(|| (Utc::now() - chrono::Duration::days(1)).to_rfc3339());
        let mut out: Vec<Value> = {
            let c = self.conn.lock().unwrap();
            let mut st = c.prepare(
                "SELECT id, 'answer', recipient, status, question, answer, room_id, answered_at FROM tickets
                   WHERE sender = ?1 AND status IN ('answered','failed') AND answered_at > ?2
                 UNION ALL
                 SELECT id, 'question', sender, status, question, NULL, room_id, created_at FROM tickets
                   WHERE recipient = ?1 AND status IN ('pending','running') AND created_at > ?2
                 ORDER BY 8 DESC LIMIT 20",
            )?;
            let rows = st.query_map(params![agent, since], |r| {
                Ok(json!({"ticket": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?,
                          "with": r.get::<_, String>(2)?, "status": r.get::<_, String>(3)?,
                          "question": r.get::<_, String>(4)?, "answer": r.get::<_, Option<String>>(5)?,
                          "room": r.get::<_, String>(6)?, "at": r.get::<_, String>(7)?}))
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        out.reverse();
        for u in out.iter_mut() {
            let w = u["with"].as_str().unwrap_or("").to_string();
            u["withName"] = json!(self.agent_name(&w));
        }
        if mark_read {
            if let Some(last) = out.last().and_then(|u| u["at"].as_str()) {
                self.set_setting(&key, last)?;
            }
        }
        Ok(out)
    }

    // ---------- settings (non-secret) ----------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let old = self.audited(key).then(|| self.setting(key).ok().flatten()).flatten();
        self.conn.lock().unwrap().execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        if self.audited(key) && old.as_deref() != Some(value) {
            self.audit("set", key, old.as_deref(), Some(value));
        }
        Ok(())
    }

    pub fn del_setting(&self, key: &str) -> Result<()> {
        let old = self.audited(key).then(|| self.setting(key).ok().flatten()).flatten();
        self.conn.lock().unwrap().execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        if old.is_some() {
            self.audit("delete", key, old.as_deref(), None);
        }
        Ok(())
    }

    /// Tunnel settings decide the public URL agents are configured with, so every change to them
    /// is recorded (none of them hold secrets; tokens live in the keychain).
    fn audited(&self, key: &str) -> bool {
        key.starts_with("tunnel.")
    }

    /// The change log, next to the vault: `<home>/logs/settings.log`.
    pub fn settings_log(&self) -> PathBuf {
        self.threads_dir.parent().unwrap_or(&self.threads_dir).join("logs").join("settings.log")
    }

    fn audit(&self, action: &str, key: &str, old: Option<&str>, new: Option<&str>) {
        use std::io::Write;
        let path = self.settings_log();
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default();
        let line = format!(
            "{} {action} {key} {:?} -> {:?} by {exe} (pid {})\n",
            now(),
            old.unwrap_or("-"),
            new.unwrap_or("-"),
            std::process::id()
        );
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(line.as_bytes());
        }
        if action == "delete" {
            tracing::info!("tunnel setting deleted: {key} (was {:?})", old.unwrap_or("-"));
        }
    }

    // ---------- context ----------

    pub fn set_context(&self, room: &str, kind: &str, content: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO context(room_id, kind, content, captured_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(room_id, kind) DO UPDATE SET content = excluded.content, captured_at = excluded.captured_at",
            params![room, kind, content, now()],
        )?;
        Ok(())
    }

    pub fn context(&self, room: &str, kind: &str) -> Result<String> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT content FROM context WHERE room_id = ?1 AND kind = ?2", params![room, kind], |r| r.get(0))
            .optional()?
            .unwrap_or_default())
    }

    // ---------- markdown export ----------

    /// Write `threads/<room>.md` atomically with 0600 permissions.
/// Consistent copy of the whole vault (safe while the app is running), for example before an
    /// update. Written as `<dir>/vault-<label>-<timestamp>.db` (0600); only the newest `keep` are kept.
    pub fn threads_dir(&self) -> &Path {
        &self.threads_dir
    }

    pub fn backup(&self, dir: &Path, label: &str, keep: usize) -> Result<PathBuf> {
        secure_dir(dir)?;
        let safe: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' }).collect();
        let path = dir.join(format!("vault-{safe}-{}.db", Utc::now().format("%Y%m%d-%H%M%S")));
        let target = path.to_string_lossy().replace('\'', "''");
        self.conn.lock().unwrap().execute_batch(&format!("VACUUM INTO '{target}'")).context("vault backup")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        let mut old: Vec<PathBuf> = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("vault-") && n.ends_with(".db")))
            .collect();
        old.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let excess = old.len().saturating_sub(keep);
        for p in old.into_iter().take(excess) {
            let _ = std::fs::remove_file(p);
        }
        Ok(path)
    }

/// Everything worth syncing, as plain JSON rows. Secrets never live in the vault, so none are here;
    /// settings are limited to an allow-list, and git diffs / terminal output only go out when asked for.
    pub fn dump_for_sync(&self, include_context: bool) -> Result<Value> {
        let c = self.conn.lock().unwrap();
        let table = |sql: &str| -> Result<Vec<Value>> {
            let mut st = c.prepare(sql)?;
            let names: Vec<String> = st.column_names().into_iter().map(|s| s.to_string()).collect();
            let rows = st.query_map([], |r| {
                let mut o = serde_json::Map::new();
                for (i, n) in names.iter().enumerate() {
                    let v = match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(x) => json!(x),
                        rusqlite::types::ValueRef::Real(x) => json!(x),
                        rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        rusqlite::types::ValueRef::Blob(_) => Value::Null,
                    };
                    o.insert(n.clone(), v);
                }
                Ok(Value::Object(o))
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        };
        let allowed = SYNC_SETTINGS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
        let kinds = if include_context { "'notes','diff','terminal'" } else { "'notes'" };
        Ok(json!({
            "providers": table("SELECT id, name, wake_kind, description, family, registered FROM providers ORDER BY rowid")?,
            "rooms": table("SELECT id, title, kind, workspace_path, branch, allow_edits, updated_at FROM rooms ORDER BY id")?,
            "room_members": table("SELECT room_id, provider_id FROM room_members ORDER BY room_id, provider_id")?,
            "sessions": table("SELECT id, room_id, title, status, started_at FROM sessions ORDER BY started_at")?,
            "messages": table("SELECT id, session_id, room_id, sender, recipient, kind, text, code, ticket_id, created_at FROM messages ORDER BY created_at, id")?,
            "tickets": table("SELECT id, session_id, room_id, sender, recipient, question, status, hop, then_to, answer, created_at, answered_at FROM tickets ORDER BY created_at, id")?,
            "context": table(&format!("SELECT room_id, kind, content, captured_at FROM context WHERE kind IN ({kinds}) ORDER BY room_id, kind"))?,
            "settings": table(&format!("SELECT key, value FROM settings WHERE key IN ({allowed}) OR key LIKE 'bot.%.can_ask' ORDER BY key"))?,
        }))
    }

    /// Merge a dump into this vault without deleting or overwriting anything local: rows are added
    /// by id, settings only fill gaps, and questions still open on the other machine are closed here.
    pub fn restore_from_sync(&self, d: &Value) -> Result<Value> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let arr = |k: &str| d[k].as_array().cloned().unwrap_or_default();
        let s = |v: &Value, k: &str| v[k].as_str().map(String::from);
        let (mut rooms, mut sessions, mut messages, mut bots) = (0, 0, 0, 0);

        for p in arr("providers") {
            let id = s(&p, "id").unwrap_or_default();
            if id.is_empty() || !(config::is_known_agent(&id) || config::is_grok_family(&id)) {
                continue;
            }
            let have: Option<i64> = tx.query_row("SELECT registered FROM providers WHERE id = ?1", params![id], |r| r.get(0)).optional()?;
            let reg = p["registered"].as_i64().unwrap_or(0);
            match have {
                None => {
                    // A webhook can't come along (its key lives in the Keychain), so restored bots start inbox-only.
                    tx.execute(
                        "INSERT INTO providers(id, name, wake_kind, calls, description, family, registered) VALUES(?1,?2,'inbox',0,?3,?4,?5)",
                        params![id, s(&p, "name").unwrap_or_else(|| id.clone()), s(&p, "description"), s(&p, "family"), reg],
                    )?;
                    bots += 1;
                }
                Some(0) if reg == 1 => {
                    tx.execute(
                        "UPDATE providers SET name = ?2, description = ?3, registered = 1 WHERE id = ?1",
                        params![id, s(&p, "name").unwrap_or_else(|| id.clone()), s(&p, "description")],
                    )?;
                }
                _ => {}
            }
        }
        for r in arr("rooms") {
            let n = tx.execute(
                "INSERT OR IGNORE INTO rooms(id, title, kind, workspace_path, branch, allow_edits, updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![s(&r, "id"), s(&r, "title"), s(&r, "kind"), s(&r, "workspace_path"), s(&r, "branch"), r["allow_edits"].as_i64().unwrap_or(0), s(&r, "updated_at")],
            )?;
            rooms += n;
        }
        for m in arr("room_members") {
            tx.execute("INSERT OR IGNORE INTO room_members(room_id, provider_id) VALUES(?1,?2)", params![s(&m, "room_id"), s(&m, "provider_id")])?;
        }
        for x in arr("sessions") {
            sessions += tx.execute(
                "INSERT OR IGNORE INTO sessions(id, room_id, title, status, started_at) VALUES(?1,?2,?3,?4,?5)",
                params![s(&x, "id"), s(&x, "room_id"), s(&x, "title"), s(&x, "status"), s(&x, "started_at")],
            )?;
        }
        for x in arr("messages") {
            messages += tx.execute(
                "INSERT OR IGNORE INTO messages(id, session_id, room_id, sender, recipient, kind, text, code, ticket_id, created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![s(&x, "id"), s(&x, "session_id"), s(&x, "room_id"), s(&x, "sender"), s(&x, "recipient"), s(&x, "kind"), s(&x, "text"), s(&x, "code"), s(&x, "ticket_id"), s(&x, "created_at")],
            )?;
        }
        for x in arr("tickets") {
            let st = s(&x, "status").unwrap_or_default();
            let st = if st == "pending" || st == "running" { "expired".to_string() } else { st };
            tx.execute(
                "INSERT OR IGNORE INTO tickets(id, session_id, room_id, sender, recipient, question, status, hop, then_to, answer, created_at, answered_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![s(&x, "id"), s(&x, "session_id"), s(&x, "room_id"), s(&x, "sender"), s(&x, "recipient"), s(&x, "question"), st, x["hop"].as_i64().unwrap_or(0), s(&x, "then_to"), s(&x, "answer"), s(&x, "created_at"), s(&x, "answered_at")],
            )?;
        }
        for x in arr("context") {
            tx.execute(
                "INSERT OR IGNORE INTO context(room_id, kind, content, captured_at) VALUES(?1,?2,?3,?4)",
                params![s(&x, "room_id"), s(&x, "kind"), s(&x, "content"), s(&x, "captured_at")],
            )?;
        }
        for x in arr("settings") {
            if let (Some(k), Some(v)) = (s(&x, "key"), s(&x, "value")) {
                if SYNC_SETTINGS.contains(&k.as_str()) || (k.starts_with("bot.") && k.ends_with(".can_ask")) {
                    tx.execute("INSERT OR IGNORE INTO settings(key, value) VALUES(?1,?2)", params![k, v])?;
                }
            }
        }
        tx.commit()?;
        Ok(json!({"rooms": rooms, "sessions": sessions, "messages": messages, "bots": bots}))
    }

    pub fn export_markdown(&self, room: &str) -> Result<PathBuf> {
        let Some(r) = self.room(room)? else { anyhow::bail!("unknown room {room}") };
        let mut md = format!(
            "# {}\n\n- workspace: `{}`\n- branch: `{}`\n- members: {}\n\n",
            r["title"].as_str().unwrap_or(""),
            r["workspacePath"].as_str().unwrap_or(""),
            r["branch"].as_str().unwrap_or(""),
            r["participants"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| format!("@{s}")).collect::<Vec<_>>().join(", ")).unwrap_or_default()
        );
        for key in ["notes", "terminal", "diff"] {
            let body = self.context(room, key)?;
            if !body.is_empty() {
                md.push_str(&format!("## Shared {key}\n\n```\n{body}\n```\n\n"));
            }
        }
        for s in r["sessions"].as_array().cloned().unwrap_or_default().iter().rev() {
            md.push_str(&format!("## {} [{}]\n\n", s["title"].as_str().unwrap_or(""), s["status"].as_str().unwrap_or("")));
            for m in self.messages(s["id"].as_str().unwrap_or(""))? {
                md.push_str(&format!(
                    "- **{}** → {}: {}\n",
                    m["sender"].as_str().unwrap_or(""),
                    m["recipient"].as_str().unwrap_or("all"),
                    m["text"].as_str().unwrap_or("").replace('\n', "\n  ")
                ));
            }
            md.push('\n');
        }
        let safe = Path::new(room).file_name().map(|f| f.to_os_string()).unwrap_or_default();
        let path = self.threads_dir.join(safe).with_extension("md");
        atomic_write(&path, &md)?;
        Ok(path)
    }
}

fn family_of(id: &str) -> String {
    crate::providers::get(id).map(|p| p.info().id).unwrap_or_else(|| id.to_string())
}

/// Add columns introduced after the first release; safe to run on every open.
fn migrate(conn: &Connection) -> Result<()> {
    let have: Vec<String> = conn
        .prepare("PRAGMA table_info(providers)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<_, _>>()?;
    for (col, ddl) in [
        ("description", "ALTER TABLE providers ADD COLUMN description TEXT"),
        ("family", "ALTER TABLE providers ADD COLUMN family TEXT"),
        ("registered", "ALTER TABLE providers ADD COLUMN registered INTEGER DEFAULT 0"),
    ] {
        if !have.iter().any(|c| c == col) {
            conn.execute(ddl, [])?;
        }
    }
    let scols: Vec<String> = conn
        .prepare("PRAGMA table_info(sessions)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<_, _>>()?;
    if !scols.iter().any(|c| c == "topic") {
        // What the session is about, normalized (see `topic_key`), so a later question on the same
        // topic lands in it.
        conn.execute("ALTER TABLE sessions ADD COLUMN topic TEXT", [])?;
    }
    conn.execute("UPDATE providers SET family = CASE WHEN id = 'grok' OR id LIKE 'grok:%' THEN 'grok' ELSE id END WHERE family IS NULL", [])?;
    Ok(())
}

fn default_wake_kind(id: &str) -> &'static str {
    // A webhook provider (Grok) has no headless CLI; until its webhook is set up it uses the inbox.
    match crate::providers::get(id).map(|p| p.caps().wake) {
        Some("cli") => "cli",
        _ => "inbox",
    }
}

fn slugify(path: &str) -> String {
    let base: String = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "room".into())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut h: u64 = 1469598103934665603;
    for b in path.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{}-{:06x}", base.trim_matches('-'), h & 0xffffff)
}

/// A user-facing name: trimmed, single-spaced, at most 60 characters. None if empty.
fn clean_title(s: &str) -> Option<String> {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(60).collect();
    (!t.is_empty()).then_some(t)
}

const TOPIC_STOPWORDS: &[&str] = &["the", "a", "an", "of", "for", "on", "about", "and", "to", "in", "our", "my", "re"];

/// A topic reduced to its words: lowercase, punctuation and filler words dropped, so "The
/// release plan!" and "release plan" are the same topic. Session-number prefixes are ignored.
pub fn topic_key(s: &str) -> String {
    let lower = s.to_lowercase();
    let lower = lower.strip_prefix("session #").map(|r| r.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ' || c == '·')).unwrap_or(&lower);
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !TOPIC_STOPWORDS.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Same topic? Equal word sets, or one name's words all inside the other's ("release" matches
/// "release plan"), but never on a single shared word between two longer names ("release plan"
/// vs "release notes" are different topics).
pub fn topics_match(a: &str, b: &str) -> bool {
    let wa: std::collections::BTreeSet<&str> = a.split(' ').filter(|w| !w.is_empty()).collect();
    let wb: std::collections::BTreeSet<&str> = b.split(' ').filter(|w| !w.is_empty()).collect();
    if wa.is_empty() || wb.is_empty() {
        return false;
    }
    wa == wb || wa.is_subset(&wb) || wb.is_subset(&wa)
}
