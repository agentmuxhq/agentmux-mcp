//! Media in conversations: images, audio, video and PDFs attached to questions, answers and notes.
//! Files are copied into `<home>/media` (0600) so they outlive the sender's temp folders; the app
//! shows and plays them, local agents get their paths, and captions are indexed with the message.

use crate::vault::Vault;
use anyhow::{anyhow, bail, Result};
use base64::Engine as _;
use rusqlite::params;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const DEFAULT_MAX_MB: u64 = 50;

pub fn dir(vault: &Vault) -> PathBuf {
    vault.threads_dir().parent().unwrap_or(vault.threads_dir()).join("media")
}

pub fn max_bytes(vault: &Vault) -> u64 {
    vault.setting("media.max_mb").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MAX_MB) * 1024 * 1024
}

/// The media type for a file name; None for types AgentMux doesn't keep.
pub fn mime_for(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_string_lossy().to_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

fn ext_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "audio/mp4" => "m4a",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        _ => "pdf",
    }
}

/// Store one attachment given as `{path}` (agents on this Mac only) or `{data: base64, name|mime}`,
/// with an optional `caption`. Returns its record.
pub fn save(vault: &Vault, session: &str, room: &str, input: &Value, local_caller: bool) -> Result<Value> {
    let cap = max_bytes(vault);
    let caption = input["caption"].as_str().map(|c| c.trim().chars().take(500).collect::<String>()).filter(|c| !c.is_empty());
    let (bytes, name, mime) = if let Some(p) = input["path"].as_str() {
        if !local_caller {
            bail!("cloud bots can't attach files by path; send the file as base64 `data`");
        }
        let path = PathBuf::from(p);
        let meta = std::fs::metadata(&path).map_err(|_| anyhow!("can't read {p}"))?;
        if !meta.is_file() {
            bail!("{p} isn't a file");
        }
        if meta.len() > cap {
            bail!("{p} is {} MB; the limit is {} MB", meta.len() / (1 << 20), cap / (1 << 20));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let mime = input["mime"].as_str().map(String::from).or_else(|| mime_for(&name).map(String::from)).ok_or_else(|| anyhow!("{name}: only images, audio, video and PDFs can be attached"))?;
        (std::fs::read(&path)?, name, mime)
    } else if let Some(d) = input["data"].as_str() {
        let d = d.split_once("base64,").map(|x| x.1).unwrap_or(d);
        if (d.len() as u64) / 4 * 3 > cap {
            bail!("the attachment is over the {} MB limit", cap / (1 << 20));
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(d.trim()).map_err(|_| anyhow!("attachment data isn't valid base64"))?;
        let name = input["name"].as_str().unwrap_or("attachment").chars().take(120).collect::<String>();
        let mime = input["mime"].as_str().map(String::from).or_else(|| mime_for(&name).map(String::from)).ok_or_else(|| anyhow!("give the attachment a `mime` or a file name with an extension"))?;
        (bytes, name, mime)
    } else {
        bail!("an attachment needs `path` or `data`");
    };
    mime_from_list(&mime)?;
    let id = crate::vault::new_id("A");
    let d = dir(vault);
    std::fs::create_dir_all(&d)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
    }
    let path = d.join(format!("{id}.{}", ext_for(&mime)));
    std::fs::write(&path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    vault.conn().execute(
        "INSERT INTO attachments(id, message_id, session_id, room_id, name, mime, size, path, caption, created_at) VALUES(?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id, session, room, name, mime, bytes.len() as i64, path.to_string_lossy(), caption, crate::vault::now()],
    )?;
    Ok(json!({"id": id, "name": name, "mime": mime, "size": bytes.len(), "path": path.to_string_lossy(), "caption": caption}))
}

fn mime_from_list(m: &str) -> Result<&'static str> {
    const ALL: &[&str] = &[
        "image/png", "image/jpeg", "image/gif", "image/webp", "image/heic", "audio/mpeg", "audio/wav", "audio/mp4", "audio/ogg",
        "video/mp4", "video/quicktime", "video/webm", "application/pdf",
    ];
    ALL.iter().copied().find(|x| *x == m).ok_or_else(|| anyhow!("unsupported media type {m}: images, audio, video and PDFs only"))
}

/// The text line a message carries for its attachments, so search finds them by name and caption.
pub fn describe(atts: &[Value]) -> String {
    atts.iter()
        .map(|a| {
            let kind = a["mime"].as_str().unwrap_or("").split('/').next().unwrap_or("file");
            let kind = if kind == "application" { "pdf" } else { kind };
            match a["caption"].as_str() {
                Some(c) => format!("[{kind}: {} — {c}]", a["name"].as_str().unwrap_or("")),
                None => format!("[{kind}: {}]", a["name"].as_str().unwrap_or("")),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Attach stored files to the message that carries them (and its thread). An attachment already
/// linked (e.g. one file sent to several agents) stays with its first message.
pub fn link(vault: &Vault, ids: &[String], message_id: &str) -> Result<()> {
    let c = vault.conn();
    for id in ids {
        c.execute(
            "UPDATE attachments SET message_id = ?2,
               session_id = (SELECT session_id FROM messages WHERE id = ?2), room_id = (SELECT room_id FROM messages WHERE id = ?2)
             WHERE id = ?1 AND message_id IS NULL",
            params![id, message_id],
        )?;
    }
    Ok(())
}

fn rows(vault: &Vault, sql: &str, key: &str) -> Result<Vec<Value>> {
    let c = vault.conn();
    let mut st = c.prepare(sql)?;
    let out = st
        .query_map(params![key], |r| {
            Ok(json!({"id": r.get::<_, String>(0)?, "message": r.get::<_, Option<String>>(1)?, "name": r.get::<_, String>(2)?,
                      "mime": r.get::<_, String>(3)?, "size": r.get::<_, i64>(4)?, "path": r.get::<_, String>(5)?,
                      "caption": r.get::<_, Option<String>>(6)?}))
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(out)
}

pub fn list_for_message(vault: &Vault, message_id: &str) -> Result<Vec<Value>> {
    rows(vault, "SELECT id, message_id, name, mime, size, path, caption FROM attachments WHERE message_id = ?1 ORDER BY created_at", message_id)
}

pub fn list_for_session(vault: &Vault, session: &str) -> Result<Vec<Value>> {
    rows(vault, "SELECT id, message_id, name, mime, size, path, caption FROM attachments WHERE session_id = ?1 ORDER BY created_at", session)
}

pub fn get(vault: &Vault, id: &str) -> Result<Option<Value>> {
    Ok(rows(vault, "SELECT id, message_id, name, mime, size, path, caption FROM attachments WHERE id = ?1", id)?.into_iter().next())
}

/// Session an attachment belongs to.
pub fn session_of(vault: &Vault, id: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(vault.conn().query_row("SELECT session_id FROM attachments WHERE id = ?1", params![id], |r| r.get(0)).optional()?)
}

/// MCP content blocks for image attachments (so multimodal agents see them), up to `max` images of ≤5 MB.
pub fn image_blocks(atts: &[Value], max: usize) -> Vec<Value> {
    atts.iter()
        .filter(|a| a["mime"].as_str().is_some_and(|m| m.starts_with("image/") && m != "image/heic"))
        .filter(|a| a["size"].as_i64().unwrap_or(i64::MAX) <= 5 * 1024 * 1024)
        .take(max)
        .filter_map(|a| {
            let bytes = std::fs::read(a["path"].as_str()?).ok()?;
            Some(json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes), "mimeType": a["mime"]}))
        })
        .collect()
}

/// Attachments of a ticket's question.
pub fn for_ticket_request(vault: &Vault, ticket: &str) -> Result<Vec<Value>> {
    rows(vault, "SELECT a.id, a.message_id, a.name, a.mime, a.size, a.path, a.caption FROM attachments a JOIN messages m ON m.id = a.message_id
                 WHERE m.ticket_id = ?1 AND m.kind = 'a2a_request' ORDER BY a.created_at", ticket)
}

/// Attachments of a ticket's answer.
pub fn for_ticket_answer(vault: &Vault, ticket: &str) -> Result<Vec<Value>> {
    rows(vault, "SELECT a.id, a.message_id, a.name, a.mime, a.size, a.path, a.caption FROM attachments a JOIN messages m ON m.id = a.message_id
                 WHERE m.ticket_id = ?1 AND m.kind = 'a2a_reply' ORDER BY a.created_at", ticket)
}
