//! Which thread a question on a topic belongs to, when its wording doesn't name a thread exactly.
//!
//!  1. the thread whose name or topic matches (`Vault::session_for_topic`);
//!  2. candidates scored by name, summary and (with an embed engine) meaning;
//!  3. the decide engine (`builtin`, `llm` or `jeff`) picks one or "new", with a confidence;
//!  4. unsure → the caller gets the candidates and asks the user in their chat.
//!
//! Every decision is logged (`route_log`), and when the user answers an "unsure", the log records
//! whether the router's best guess was right. That agreement rate says whether a stronger decision
//! model (Jeff) is worth turning on.

use crate::{engines, memory, vault::Vault};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub enum Route {
    /// Continue this thread.
    Use { session: String, method: &'static str, confidence: f64 },
    /// Start a new thread named after the topic.
    New,
    /// Unsure: let the user pick from these (`session`, `thread`, `why`).
    Ask { options: Vec<Value> },
}

/// Auto-route at or above this; ask between `ASK_FLOOR` and here; a new thread below the floor.
const SURE: f64 = 0.7;
const ASK_FLOOR: f64 = 0.25;

fn topic_words(t: &str) -> Vec<String> {
    crate::vault::topic_key(t).split(' ').filter(|w| w.chars().count() >= 2).map(String::from).collect()
}

struct Candidate {
    session: String,
    title: String,
    state: String,
    score: f64,
}

async fn candidates(vault: &Vault, room: &str, topic: &str) -> Result<Vec<Candidate>> {
    let ws = topic_words(topic);
    let rows: Vec<(String, String, Option<String>, Option<String>, Option<Vec<u8>>)> = {
        let c = vault.conn();
        let mut st = c.prepare(
            "SELECT s.id, s.title, s.topic, m.content, m.vec FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id
             WHERE s.room_id = ?1 AND EXISTS (SELECT 1 FROM messages x WHERE x.session_id = s.id AND x.kind != 'system')",
        )?;
        st.query_map(params![room], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<std::result::Result<_, _>>()?
    };
    let qv = engines::embed(vault, &[topic.to_string()]).await.ok().flatten().map(|(_, v)| v[0].clone());
    let mut out = vec![];
    for (session, title, tkey, summary, vec) in rows {
        let name = format!("{} {}", crate::vault::topic_key(&title), tkey.unwrap_or_default());
        let sum = summary.clone().unwrap_or_default().to_lowercase();
        let n = ws.len().max(1) as f64;
        let in_name = ws.iter().filter(|w| name.split(' ').any(|x| x == w.as_str())).count() as f64 / n;
        let in_sum = ws.iter().filter(|w| sum.contains(w.as_str())).count() as f64 / n;
        let mut score = in_name * 0.75 + in_sum * 0.35;
        if let (Some(q), Some(b)) = (&qv, vec) {
            let sim = engines::cosine(q, &memory::from_blob(&b)) as f64;
            score = score.max(((sim - 0.3) / 0.5).clamp(0.0, 1.0));
        }
        let state = summary.as_deref().map(|s| current_state(s)).unwrap_or_default();
        out.push(Candidate { session, title, state, score: score.min(1.0) });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

fn current_state(summary: &str) -> String {
    let mut it = summary.lines().skip_while(|l| !l.contains("Current state")).skip(1);
    it.find(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| memory::clip(l.trim_start_matches(['-', '*', ' ']), 160)).unwrap_or_default()
}

fn option(c: &Candidate) -> Value {
    json!({"session": c.session, "thread": c.title, "currentState": c.state, "match": (c.score * 100.0).round() / 100.0})
}

/// Decide where a question on `topic` goes in `room`. `can_ask`: whether the caller can put the
/// choice to a user (cloud bots can't).
pub async fn route(vault: &Vault, agent: &str, room: &str, topic: &str, question: &str, can_ask: bool) -> Result<Route> {
    if let Some(s) = vault.session_for_topic(room, topic)? {
        log(vault, agent, room, topic, "topic", Some(&s), Some(&s), 1.0, "rules", &[])?;
        return Ok(Route::Use { session: s, method: "topic", confidence: 1.0 });
    }
    let cands = candidates(vault, room, topic).await?;
    let top = cands.first();
    let top_score = top.map(|c| c.score).unwrap_or(0.0);
    // With a decision engine on, it judges meaning itself, so it sees the threads even when no
    // words overlap; without one, a weak best match means a new topic.
    let engine_on = engines::Engine::load(vault, "decide").enabled();
    if cands.is_empty() || (top_score < ASK_FLOOR && !engine_on) {
        log(vault, agent, room, topic, "new", None, top.map(|c| c.session.as_str()), top_score, "rules", &[])?;
        return Ok(Route::New);
    }
    let shortlist: Vec<&Candidate> = cands.iter().take(4).collect();
    let options: Vec<Value> = shortlist.iter().map(|c| option(c)).collect();

    // A decision engine, when one is configured.
    let mut choices: Vec<(String, String)> = shortlist
        .iter()
        .enumerate()
        .map(|(i, c)| (format!("t{}", i + 1), if c.state.is_empty() { c.title.clone() } else { format!("{}: {}", c.title, c.state) }))
        .collect();
    choices.push(("new".into(), "A new topic, different from all of these threads".into()));
    let state = format!("Topic: {topic}\nQuestion: {}", memory::clip(question, 1500));
    match engines::choose(vault, "decide", &state, "Which existing thread does this belong to, or is it a new topic?", &choices).await {
        Ok(Some(d)) => {
            let picked = d.key.strip_prefix('t').and_then(|n| n.parse::<usize>().ok()).and_then(|n| shortlist.get(n - 1));
            if d.confidence >= SURE || d.probability >= 0.85 {
                return Ok(match picked {
                    Some(c) => {
                        log(vault, agent, room, topic, "decided", Some(&c.session), top.map(|c| c.session.as_str()), d.confidence, &d.engine, &options)?;
                        Route::Use { session: c.session.clone(), method: "decided", confidence: d.confidence }
                    }
                    None => {
                        log(vault, agent, room, topic, "new", None, top.map(|c| c.session.as_str()), d.confidence, &d.engine, &options)?;
                        Route::New
                    }
                });
            }
            return unsure(vault, agent, room, topic, &cands, options, can_ask, d.confidence, &d.engine);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("decide engine failed, using the built-in rules: {e}"),
    }
    // Built in: sure when the best match is strong and clearly ahead of the next.
    let second = cands.get(1).map(|c| c.score).unwrap_or(0.0);
    if top_score >= SURE && top_score - second >= 0.2 {
        let s = &cands[0].session;
        log(vault, agent, room, topic, "semantic", Some(s), Some(s), top_score, "rules", &options)?;
        return Ok(Route::Use { session: s.clone(), method: "semantic", confidence: top_score });
    }
    unsure(vault, agent, room, topic, &cands, options, can_ask, top_score, "rules")
}

#[allow(clippy::too_many_arguments)]
fn unsure(vault: &Vault, agent: &str, room: &str, topic: &str, cands: &[Candidate], options: Vec<Value>, can_ask: bool, conf: f64, engine: &str) -> Result<Route> {
    let top = cands.first().map(|c| c.session.as_str());
    if can_ask {
        log(vault, agent, room, topic, "asked", None, top, conf, engine, &options)?;
        return Ok(Route::Ask { options });
    }
    // Nobody to ask: the best match if it's reasonable, else a new thread.
    match cands.first().filter(|c| c.score >= 0.5) {
        Some(c) => {
            log(vault, agent, room, topic, "best-guess", Some(&c.session), top, conf, engine, &options)?;
            Ok(Route::Use { session: c.session.clone(), method: "best-guess", confidence: c.score })
        }
        None => {
            log(vault, agent, room, topic, "new", None, top, conf, engine, &options)?;
            Ok(Route::New)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn log(vault: &Vault, agent: &str, room: &str, topic: &str, method: &str, chosen: Option<&str>, top: Option<&str>, conf: f64, engine: &str, options: &[Value]) -> Result<()> {
    vault.conn().execute(
        "INSERT INTO route_log(at, agent, room_id, topic, method, chosen, top, confidence, engine, options) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![crate::vault::now(), agent, room, topic, method, chosen, top, conf, engine, serde_json::to_string(options)?],
    )?;
    Ok(())
}

/// The user answered an "unsure": record what they picked (a session id, or "new").
pub fn resolve(vault: &Vault, agent: &str, room: &str, topic: &str, picked: &str) -> Result<bool> {
    let id: Option<i64> = vault
        .conn()
        .query_row(
            "SELECT id FROM route_log WHERE agent = ?1 AND room_id = ?2 AND topic = ?3 AND method = 'asked' AND resolved IS NULL
             AND at >= ?4 ORDER BY id DESC LIMIT 1",
            params![agent, room, topic, (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else { return Ok(false) };
    vault.conn().execute("UPDATE route_log SET resolved = ?2 WHERE id = ?1", params![id, picked])?;
    Ok(true)
}

/// How routing is doing, and whether a stronger decision model would help.
pub fn stats(vault: &Vault) -> Result<Value> {
    let c = vault.conn();
    let mut by_method = serde_json::Map::new();
    let mut st = c.prepare("SELECT method, COUNT(*) FROM route_log GROUP BY method")?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (m, n) = row?;
        by_method.insert(m, json!(n));
    }
    let total: i64 = c.query_row("SELECT COUNT(*) FROM route_log", [], |r| r.get(0))?;
    let asked: i64 = c.query_row("SELECT COUNT(*) FROM route_log WHERE method = 'asked'", [], |r| r.get(0))?;
    let resolved: i64 = c.query_row("SELECT COUNT(*) FROM route_log WHERE method = 'asked' AND resolved IS NOT NULL", [], |r| r.get(0))?;
    let agreed: i64 = c.query_row("SELECT COUNT(*) FROM route_log WHERE method = 'asked' AND resolved IS NOT NULL AND resolved = top", [], |r| r.get(0))?;
    let decided: i64 = c.query_row("SELECT COUNT(*) FROM route_log WHERE engine IN ('jeff', 'llm')", [], |r| r.get(0))?;
    let agreement = if resolved > 0 { Some(agreed as f64 / resolved as f64) } else { None };
    let advice = match (agreement, resolved) {
        (_, n) if n < 10 => "Not enough answered questions yet to judge (need about 10).".to_string(),
        (Some(a), _) if a >= 0.8 => format!("The router's best guess matched your answer {:.0}% of the time: it could decide these itself. A decision engine (Jeff or the local LLM) would let it, with fewer questions to you.", a * 100.0),
        (Some(a), _) => format!("The router's best guess matched your answer only {:.0}% of the time: keep asking, or try Jeff and compare.", a * 100.0),
        _ => String::new(),
    };
    Ok(json!({"total": total, "byMethod": by_method, "asked": asked, "answered": resolved, "agreedWithRouter": agreed,
              "agreement": agreement, "byDecisionEngine": decided, "advice": advice,
              "askRate": if total > 0 { Some(asked as f64 / total as f64) } else { None }}))
}
