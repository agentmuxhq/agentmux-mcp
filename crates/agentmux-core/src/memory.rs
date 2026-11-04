//! Memory: everything agents said, searchable across products, with a contextual summary per
//! thread and a brief per product, kept current in the background.
//!
//! Search works without any engine (exact words, ranked, plus thread names and summaries). With an
//! embed engine it also finds meaning ("the animation handoff" ↔ "moving the sprite pipeline").
//! Summaries are written by the writer engine when one is enabled; otherwise a structured outline
//! is built from the thread itself, so there is always something to hand over.

use crate::{engines, vault::Vault};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Which threads a caller may search. Agents on this Mac see every product; a cloud bot only
/// the threads it took part in.
#[derive(Clone, Debug)]
pub enum Scope {
    All,
    Participant(String),
}

fn words(q: &str) -> Vec<String> {
    let mut seen = vec![];
    for w in q.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if w.chars().count() >= 2 && !STOP.contains(&w) && !seen.iter().any(|s: &String| s == w) {
            seen.push(w.to_string());
        }
    }
    seen
}

const STOP: &[&str] = &[
    "the", "a", "an", "of", "for", "on", "about", "and", "to", "in", "is", "it", "what", "whats", "happened", "with", "was", "did", "do",
    "does", "how", "where", "when", "who", "why", "that", "this", "any", "our", "my", "we", "you", "me", "at", "be", "are", "from",
];

fn allowed_sessions(vault: &Vault, scope: &Scope) -> Result<Option<Vec<String>>> {
    match scope {
        Scope::All => Ok(None),
        Scope::Participant(agent) => {
            let c = vault.conn();
            let mut st = c.prepare("SELECT DISTINCT session_id FROM tickets WHERE sender = ?1 OR recipient = ?1")?;
            Ok(Some(st.query_map(params![agent], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?))
        }
    }
}

/// Threads matching `query`, best first: room, thread, why it matched, and the best snippets.
pub async fn search(vault: &Vault, query: &str, scope: &Scope, limit: usize) -> Result<Vec<Value>> {
    let ws = words(query);
    if ws.is_empty() {
        return Ok(vec![]);
    }
    let allowed = allowed_sessions(vault, scope)?;
    let ok = |s: &str| allowed.as_ref().is_none_or(|a| a.iter().any(|x| x == s));
    // session -> (score, best snippets [(message id, sender, text, at)])
    let mut score: HashMap<String, f64> = HashMap::new();
    let mut snips: HashMap<String, Vec<(String, String, String, String)>> = HashMap::new();
    let mut why: HashMap<String, Vec<&'static str>> = HashMap::new();

    // 1. Exact words in messages, ranked by FTS5 (bm25).
    {
        let fts = ws.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(" OR ");
        let c = vault.conn();
        let mut st = c.prepare(
            "SELECT m.id, m.session_id, m.sender, m.text, m.created_at FROM messages_fts f
             JOIN messages m ON m.rowid = f.rowid WHERE messages_fts MATCH ?1 ORDER BY bm25(messages_fts) LIMIT 300",
        )?;
        let rows: Vec<(String, String, String, String, String)> =
            st.query_map(params![fts], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<std::result::Result<_, _>>()?;
        let mut rank = 0usize;
        for (id, sess, sender, text, at) in rows {
            if !ok(&sess) {
                continue;
            }
            rank += 1;
            // Weight by how many query words the message has.
            let lower = text.to_lowercase();
            let cover = ws.iter().filter(|w| lower.contains(w.as_str())).count() as f64 / ws.len() as f64;
            *score.entry(sess.clone()).or_default() += cover / (10.0 + rank as f64);
            let v = snips.entry(sess.clone()).or_default();
            if v.len() < 3 {
                v.push((id, sender, text, at));
            }
            why.entry(sess).or_default().push("words");
        }
    }

    // 2. Thread names, topics and summaries.
    {
        let c = vault.conn();
        let mut st = c.prepare("SELECT s.id, s.title, COALESCE(s.topic, ''), COALESCE(m.content, '') FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id")?;
        let rows: Vec<(String, String, String, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<std::result::Result<_, _>>()?;
        for (sess, title, topic, summary) in rows {
            if !ok(&sess) {
                continue;
            }
            let name = format!("{} {}", title.to_lowercase(), topic);
            let in_name = ws.iter().filter(|w| name.contains(w.as_str())).count() as f64 / ws.len() as f64;
            let sum = summary.to_lowercase();
            let in_sum = ws.iter().filter(|w| sum.contains(w.as_str())).count() as f64 / ws.len() as f64;
            let s = in_name * 0.6 + in_sum * 0.25;
            if s > 0.0 {
                *score.entry(sess.clone()).or_default() += s;
                if in_name > 0.0 {
                    why.entry(sess.clone()).or_default().push("thread name");
                }
                if in_sum > 0.0 {
                    why.entry(sess).or_default().push("summary");
                }
            }
        }
    }

    // 3. Meaning, when an embed engine is on.
    if let Ok(Some((model, qv))) = engines::embed(vault, &[query.to_string()]).await {
        let q = &qv[0];
        let rows: Vec<(String, String, String, String, String, Vec<u8>)> = {
            let c = vault.conn();
            let mut st = c.prepare(
                "SELECT e.message_id, e.session_id, m.sender, m.text, m.created_at, e.vec FROM embeddings e
                 JOIN messages m ON m.id = e.message_id WHERE e.model = ?1",
            )?;
            st.query_map(params![model], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<std::result::Result<_, _>>()?
        };
        let mut hits: Vec<(f32, String, String, String, String, String)> = rows
            .into_iter()
            .filter(|r| ok(&r.1))
            .map(|(id, sess, sender, text, at, blob)| (engines::cosine(q, &from_blob(&blob)), id, sess, sender, text, at))
            .filter(|h| h.0 > 0.35)
            .collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (i, (sim, id, sess, sender, text, at)) in hits.into_iter().take(100).enumerate() {
            *score.entry(sess.clone()).or_default() += sim as f64 * 0.5 / (1.0 + i as f64 * 0.05);
            let v = snips.entry(sess.clone()).or_default();
            if v.len() < 3 && !v.iter().any(|x| x.0 == id) {
                v.push((id, sender, text, at));
            }
            why.entry(sess).or_default().push("meaning");
        }
        // Thread summaries by meaning too.
        let rows: Vec<(String, Vec<u8>)> = {
            let c = vault.conn();
            let mut st = c.prepare("SELECT session_id, vec FROM summaries WHERE model = ?1 AND vec IS NOT NULL")?;
            st.query_map(params![model], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?
        };
        for (sess, blob) in rows {
            let sim = engines::cosine(q, &from_blob(&blob));
            if ok(&sess) && sim > 0.35 {
                *score.entry(sess.clone()).or_default() += sim as f64 * 0.6;
                why.entry(sess).or_default().push("meaning");
            }
        }
    }

    let mut ranked: Vec<(String, f64)> = score.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut out = vec![];
    for (sess, sc) in ranked.into_iter().take(limit) {
        let Some(room) = vault.session_room(&sess)? else { continue };
        let room_name = vault.room(&room)?.and_then(|r| r["title"].as_str().map(String::from)).unwrap_or_else(|| room.clone());
        let (title, last): (String, Option<String>) = vault.conn().query_row(
            "SELECT title, (SELECT MAX(created_at) FROM messages WHERE session_id = ?1) FROM sessions WHERE id = ?1",
            params![sess],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut reasons = why.remove(&sess).unwrap_or_default();
        reasons.sort();
        reasons.dedup();
        let summary = summary_of(vault, &sess)?.map(|s| first_section_line(&s, "Current state").unwrap_or_else(|| clip(&s, 200)));
        out.push(json!({
            "room": room, "roomName": room_name, "session": sess, "thread": title, "score": (sc * 1000.0).round() / 1000.0,
            "matched": reasons, "lastActivity": last, "currentState": summary,
            "snippets": snips.remove(&sess).unwrap_or_default().into_iter()
                .map(|(id, sender, text, at)| json!({"message": id, "from": sender, "at": at, "text": clip(&text, 400)})).collect::<Vec<_>>(),
        }));
    }
    Ok(out)
}

pub fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn first_section_line(summary: &str, heading: &str) -> Option<String> {
    let mut lines = summary.lines().skip_while(|l| !l.trim_start_matches('#').trim().eq_ignore_ascii_case(heading)).skip(1);
    lines.find(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| clip(l.trim_start_matches(['-', '*', ' ']), 240))
}

pub fn summary_of(vault: &Vault, session: &str) -> Result<Option<String>> {
    Ok(vault.conn().query_row("SELECT content FROM summaries WHERE session_id = ?1", params![session], |r| r.get(0)).optional()?)
}

pub fn brief_of(vault: &Vault, room: &str) -> Result<Option<String>> {
    Ok(vault.conn().query_row("SELECT content FROM briefs WHERE room_id = ?1", params![room], |r| r.get(0)).optional()?)
}

// ───────── contextual summaries ─────────

pub const SUMMARY_HEADINGS: [&str; 6] = ["Goal", "Decisions", "Current state", "Open items", "Who's involved", "Key artifacts"];

const WRITER_SYSTEM: &str = "You keep the working memory of a conversation between AI agents and a developer. \
Write a contextual summary that lets an agent who joins cold continue the work: not a recap of who said what, \
but the goal, what was decided and why, where things stand, what is still open, who is involved, and the files, \
links, tickets and commands that matter. Use exactly these markdown headings, in this order: \
## Goal, ## Decisions, ## Current state, ## Open items, ## Who's involved, ## Key artifacts. \
Bullets under each; write 'None yet.' when a section is empty. Under 350 words. Facts from the conversation only.";

fn thread_meta(vault: &Vault, session: &str) -> Result<(String, String, String)> {
    let (room, title): (String, String) =
        vault.conn().query_row("SELECT room_id, title FROM sessions WHERE id = ?1", params![session], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let room_name = vault.room(&room)?.and_then(|r| r["title"].as_str().map(String::from)).unwrap_or_else(|| room.clone());
    Ok((room, room_name, title))
}

fn transcript(vault: &Vault, msgs: &[Value], budget: usize) -> String {
    let mut out = String::new();
    for m in msgs.iter().rev() {
        let who = m["sender"].as_str().map(|s| if s == "user" { "You".to_string() } else { vault.agent_name(s) }).unwrap_or_default();
        let to = m["recipient"].as_str().filter(|r| *r != "all").map(|r| format!(" → {}", vault.agent_name(r))).unwrap_or_default();
        let line = format!("[{}] {who}{to}: {}\n", m["createdAt"].as_str().unwrap_or("").get(..16).unwrap_or(""), clip(m["text"].as_str().unwrap_or(""), 1500));
        if out.len() + line.len() > budget {
            break;
        }
        out.insert_str(0, &line);
    }
    out
}

/// Bring one thread's summary up to date. Returns (summary, engine used).
pub async fn summarize(vault: &Vault, session: &str) -> Result<(String, String)> {
    let msgs: Vec<Value> = vault.messages(session)?.into_iter().filter(|m| m["type"] != "system").collect();
    let (room, room_name, title) = thread_meta(vault, session)?;
    let (prev, prev_count): (Option<String>, i64) = vault
        .conn()
        .query_row("SELECT content, msg_count FROM summaries WHERE session_id = ?1", params![session], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .map(|(c, n): (String, i64)| (Some(c), n))
        .unwrap_or((None, 0));
    let writer = engines::Engine::load(vault, "writer");
    let (content, engine) = if writer.enabled() && !msgs.is_empty() {
        let new: Vec<Value> = msgs.iter().skip(prev_count.max(0) as usize).cloned().collect();
        let prompt = format!(
            "Product: {room_name}\nThread: {title}\n\n{}Conversation{}:\n{}",
            prev.as_deref().map(|p| format!("Current summary (update it; keep what still holds):\n{p}\n\n")).unwrap_or_default(),
            if prev.is_some() { " since then" } else { "" },
            transcript(vault, if new.is_empty() { &msgs } else { &new }, 24_000),
        );
        match engines::complete(vault, WRITER_SYSTEM, &prompt).await {
            Ok(t) if SUMMARY_HEADINGS.iter().filter(|h| t.contains(&format!("## {h}"))).count() >= 4 => (t, format!("{}:{}", writer.kind, writer.model)),
            Ok(_) | Err(_) => (outline(vault, session, &msgs, &title)?, "outline".to_string()),
        }
    } else {
        (outline(vault, session, &msgs, &title)?, "outline".to_string())
    };
    let vec = engines::embed(vault, &[format!("{title}\n{content}")]).await.ok().flatten();
    vault.conn().execute(
        "INSERT INTO summaries(session_id, room_id, content, msg_count, engine, updated_at, model, vec) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(session_id) DO UPDATE SET room_id = excluded.room_id, content = excluded.content, msg_count = excluded.msg_count,
           engine = excluded.engine, updated_at = excluded.updated_at, model = excluded.model, vec = excluded.vec",
        params![session, room, content, msgs.len() as i64, engine, crate::vault::now(), vec.as_ref().map(|v| v.0.clone()), vec.as_ref().map(|v| to_blob(&v.1[0]))],
    )?;
    Ok((content, engine))
}

/// A structured summary built from the thread itself, used when no writer engine is enabled.
fn outline(vault: &Vault, session: &str, msgs: &[Value], title: &str) -> Result<String> {
    let name = |s: &str| if s == "user" { "You".to_string() } else { vault.agent_name(s) };
    let goal = msgs.iter().find(|m| matches!(m["type"].as_str(), Some("a2a_request") | Some("context_sync"))).or(msgs.first());
    let mut who: Vec<String> = vec![];
    for m in msgs {
        for k in ["sender", "recipient"] {
            if let Some(s) = m[k].as_str().filter(|s| !matches!(*s, "all" | "agentmux")) {
                let n = name(s);
                if !who.contains(&n) {
                    who.push(n);
                }
            }
        }
    }
    let decisions: Vec<String> = msgs
        .iter()
        .filter(|m| m["type"] == "a2a_reply")
        .filter_map(|m| m["text"].as_str())
        .flat_map(|t| t.split(['\n', '.']).map(str::trim).filter(|l| l.len() > 12).collect::<Vec<_>>())
        .filter(|l| {
            let l = l.to_lowercase();
            ["decid", "agreed", "we will", "we'll", "going with", "chose", "use ", "should "].iter().any(|k| l.contains(k))
        })
        .map(|l| format!("- {}", clip(l, 200)))
        .take(6)
        .collect();
    // Read the rows first: naming agents needs the vault lock again.
    let open_rows: Vec<(String, String, String)> = {
        let c = vault.conn();
        let mut st = c.prepare("SELECT sender, recipient, question FROM tickets WHERE session_id = ?1 AND status IN ('pending','running','expired')")?;
        st.query_map(params![session], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
            .filter_map(|r| r.ok())
            .collect()
    };
    let open: Vec<String> = open_rows.into_iter().map(|(f, t, q)| format!("- {} is waiting on {}: {}", name(&f), name(&t), clip(&q, 160))).collect();
    let mut artifacts: Vec<String> = vec![];
    for m in msgs {
        for tok in m["text"].as_str().unwrap_or("").split_whitespace() {
            let t = tok.trim_matches(|c: char| matches!(c, '`' | '(' | ')' | ',' | '"' | '\'' | '<' | '>'));
            let looks = t.starts_with("http") || (t.contains('/') && t.contains('.') && t.len() > 4 && !t.starts_with("//")) || t.starts_with("T-");
            if looks && !artifacts.iter().any(|a| a == t) && artifacts.len() < 12 {
                artifacts.push(t.to_string());
            }
        }
    }
    let state: Vec<String> = msgs.iter().rev().take(3).collect::<Vec<_>>().into_iter().rev()
        .map(|m| format!("- {}: {}", name(m["sender"].as_str().unwrap_or("")), clip(m["text"].as_str().unwrap_or(""), 220)))
        .collect();
    let or_none = |v: Vec<String>| if v.is_empty() { "None yet.".to_string() } else { v.join("\n") };
    Ok(format!(
        "## Goal\n{}\n\n## Decisions\n{}\n\n## Current state\n{}\n\n## Open items\n{}\n\n## Who's involved\n{}\n\n## Key artifacts\n{}\n\n_Outline built from the thread \"{title}\". Turn on a writer engine for a written summary._",
        goal.map(|g| format!("- {}", clip(g["text"].as_str().unwrap_or(""), 300))).unwrap_or_else(|| "None yet.".into()),
        or_none(decisions),
        or_none(state),
        or_none(open),
        if who.is_empty() { "None yet.".into() } else { who.iter().map(|w| format!("- {w}")).collect::<Vec<_>>().join("\n") },
        or_none(artifacts.into_iter().map(|a| format!("- {a}")).collect()),
    ))
}

/// A product's brief: what the product is about right now, from its threads' summaries.
pub async fn brief(vault: &Vault, room: &str) -> Result<String> {
    let room_name = vault.room(room)?.and_then(|r| r["title"].as_str().map(String::from)).unwrap_or_else(|| room.to_string());
    let threads: Vec<(String, String, Option<String>, Option<String>)> = {
        let c = vault.conn();
        let mut st = c.prepare(
            "SELECT s.id, s.title, m.content, (SELECT MAX(created_at) FROM messages x WHERE x.session_id = s.id) AS last
             FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id WHERE s.room_id = ?1 ORDER BY last DESC",
        )?;
        st.query_map(params![room], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<std::result::Result<_, _>>()?
    };
    let lines: Vec<String> = threads
        .iter()
        .filter(|t| t.3.is_some())
        .map(|(_, title, sum, last)| {
            let state = sum.as_deref().and_then(|s| first_section_line(s, "Current state")).unwrap_or_else(|| "no summary yet".into());
            format!("- **{title}** ({}): {state}", last.as_deref().and_then(|l| l.get(..10)).unwrap_or(""))
        })
        .collect();
    let base = format!("# {room_name}\n\n## Threads\n{}", if lines.is_empty() { "None yet.".into() } else { lines.join("\n") });
    let writer = engines::Engine::load(vault, "writer");
    let content = if writer.enabled() && !lines.is_empty() {
        let sums: String = threads.iter().filter_map(|t| t.2.as_ref().map(|s| format!("### {}\n{}\n", t.1, clip(s, 1500)))).collect();
        match engines::complete(
            vault,
            "Write a short product brief (under 150 words) from these thread summaries: what the product is, what is being worked on, the main open items. Facts only, no headings.",
            &format!("Product: {room_name}\n\n{sums}"),
        )
        .await
        {
            Ok(t) => format!("# {room_name}\n\n{t}\n\n## Threads\n{}", lines.join("\n")),
            Err(_) => base,
        }
    } else {
        base
    };
    vault.conn().execute(
        "INSERT INTO briefs(room_id, content, updated_at) VALUES(?1,?2,?3) ON CONFLICT(room_id) DO UPDATE SET content = excluded.content, updated_at = excluded.updated_at",
        params![room, content, crate::vault::now()],
    )?;
    Ok(content)
}

// ───────── background upkeep ─────────

/// One pass of upkeep: embed new messages (when an embed engine is on), refresh summaries of
/// threads that changed (and have been quiet for `quiet_secs`), then the briefs of their products.
/// Returns how many summaries were refreshed.
pub async fn tick(vault: &Vault, quiet_secs: i64, max_summaries: usize) -> Result<usize> {
    // Embeddings, in batches.
    loop {
        let pending: Vec<(String, String, String, String)> = {
            let c = vault.conn();
            let mut st = c.prepare(
                "SELECT m.id, m.session_id, m.room_id, m.text FROM messages m LEFT JOIN embeddings e ON e.message_id = m.id
                 WHERE e.message_id IS NULL AND length(trim(m.text)) > 0 AND m.kind != 'system' LIMIT 32",
            )?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<std::result::Result<_, _>>()?
        };
        if pending.is_empty() {
            break;
        }
        let texts: Vec<String> = pending.iter().map(|p| clip(&p.3, 4000)).collect();
        let Some((model, vecs)) = engines::embed(vault, &texts).await? else { break };
        let c = vault.conn();
        for (p, v) in pending.iter().zip(vecs) {
            c.execute(
                "INSERT OR REPLACE INTO embeddings(message_id, session_id, room_id, model, vec) VALUES(?1,?2,?3,?4,?5)",
                params![p.0, p.1, p.2, model, to_blob(&v)],
            )?;
        }
    }
    // Summaries of changed, quiet threads.
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(quiet_secs)).to_rfc3339();
    let stale: Vec<(String, String)> = {
        let c = vault.conn();
        let mut st = c.prepare(
            "SELECT s.id, s.room_id FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id
             WHERE (SELECT COUNT(*) FROM messages x WHERE x.session_id = s.id AND x.kind != 'system') > COALESCE(m.msg_count, 0)
               AND (SELECT MAX(created_at) FROM messages x WHERE x.session_id = s.id) <= ?1
             LIMIT ?2",
        )?;
        st.query_map(params![cutoff, max_summaries as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?
    };
    let mut rooms: Vec<String> = vec![];
    for (sess, room) in &stale {
        if let Err(e) = summarize(vault, sess).await {
            tracing::warn!("summary of {sess} failed: {e}");
        }
        if !rooms.contains(room) {
            rooms.push(room.clone());
        }
    }
    for r in rooms {
        let _ = brief(vault, &r).await;
    }
    Ok(stale.len())
}
