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
        for (id, name) in config::AGENTS {
            v.conn.lock().unwrap().execute(
                "INSERT OR IGNORE INTO providers(id, name, wake_kind, calls, family) VALUES(?1, ?2, ?3, 0, ?4)",
                params![id, name, default_wake_kind(id), family_of(id)],
            )?;
        }
        Ok(v)
    }

    // ---------- providers ----------

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
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// A fixed agent or a bot that exists in `providers`.
    pub fn is_agent(&self, id: &str) -> bool {
        config::AGENTS.iter().any(|(a, _)| *a == id)
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
        let title = Path::new(workspace)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| workspace.to_string());
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

    pub fn new_session(&self, room: &str, title: &str) -> Result<String> {
        let c = self.conn.lock().unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM sessions WHERE room_id = ?1", params![room], |r| r.get(0))?;
        let sid = new_id("sess");
        let title = if title.trim().is_empty() {
            format!("Session #{}", n + 1)
        } else {
            format!("Session #{} · {}", n + 1, title.trim())
        };
        c.execute(
            "INSERT INTO sessions(id, room_id, title, status, started_at) VALUES(?1, ?2, ?3, 'Active', ?4)",
            params![sid, room, title, now()],
        )?;
        Ok(sid)
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

    /// Have `a` and `b` asked each other anything in this session since `since` (RFC 3339)?
    pub fn pair_active_since(&self, session: &str, a: &str, b: &str, since: &str) -> Result<bool> {
        let n: i64 = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM tickets WHERE session_id = ?1 AND created_at >= ?4
               AND ((sender = ?2 AND recipient = ?3) OR (sender = ?3 AND recipient = ?2))",
            params![session, a, b, since],
            |r| r.get(0),
        )?;
        Ok(n > 0)
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
        let mut st = c.prepare("SELECT id, title, status, started_at FROM sessions WHERE room_id = ?1 ORDER BY started_at DESC")?;
        let sessions: Vec<Value> = st
            .query_map(params![room], |r| {
                Ok(json!({"id": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?,
                          "status": r.get::<_, String>(2)?, "startedAt": r.get::<_, String>(3)?}))
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
        self.conn.lock().unwrap().execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn del_setting(&self, key: &str) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
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

fn family_of(id: &str) -> &str {
    if config::is_grok_family(id) { "grok" } else { id }
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
    conn.execute("UPDATE providers SET family = CASE WHEN id = 'grok' OR id LIKE 'grok:%' THEN 'grok' ELSE id END WHERE family IS NULL", [])?;
    Ok(())
}

fn default_wake_kind(id: &str) -> &'static str {
    match id {
        // Grok has no headless CLI; until its webhook is configured it picks questions up via mux_inbox.
        id if config::is_grok_family(id) => "inbox",
        _ => "cli",
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
