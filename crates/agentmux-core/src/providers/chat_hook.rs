//! Chat hooks: agents whose chat client runs a command on each message (Claude Code's
//! UserPromptSubmit, Gemini CLI's BeforeAgent) and when a chat starts. AgentMux uses them to
//!  * put late replies and notices into the same chat, on the user's next message;
//!  * remember which chat is active in which folder, so its questions continue its own thread;
//!  * tell a chat, when it (re)starts, which AgentMux thread it's on and where that stands.

use super::*;

/// How one client wires its hooks.
pub struct HookSpec {
    /// AgentMux agent id ("claude", "gemini").
    pub agent: &'static str,
    /// Events to register: the per-message one and the chat-start one.
    pub events: &'static [&'static str],
    /// The chat-start event name, which gets the thread briefing.
    pub start_event: &'static str,
    /// Our command ends with this, so we can find and remove only our entries.
    pub mark: &'static str,
    /// Give our entries a `"name"` (Gemini CLI shows it; Claude Code's schema has no such field).
    pub named: bool,
}

fn is_ours(spec: &HookSpec, h: &Value) -> bool {
    h["command"].as_str().is_some_and(|c| c.contains("agentmux") && c.ends_with(spec.mark))
}

fn quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.contains(' ') { format!("'{s}'") } else { s.to_string() }
}

/// Add (or, with `link = None`, remove) our hook entries in a client's settings file, leaving
/// everything else alone. The first edit keeps a `.agentmux.bak` backup.
pub fn edit_hooks(spec: &HookSpec, path: &Path, link: Option<&Path>) -> Result<()> {
    use crate::vault::atomic_write;
    let mut cfg: Value = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        let bak = path.with_extension("json.agentmux.bak");
        if !bak.exists() {
            atomic_write(&bak, &raw)?;
        }
        serde_json::from_str(&raw).map_err(|_| anyhow::anyhow!("{} is not valid JSON; refusing to edit", path.display()))?
    } else if link.is_some() {
        std::fs::create_dir_all(path.parent().unwrap())?;
        json!({})
    } else {
        return Ok(());
    };
    if !cfg["hooks"].is_object() {
        cfg["hooks"] = json!({});
    }
    for ev in spec.events {
        let mut groups: Vec<Value> = cfg["hooks"][*ev].as_array().cloned().unwrap_or_default();
        // Drop our old entries first so a re-install never duplicates.
        for g in groups.iter_mut() {
            if let Some(hs) = g["hooks"].as_array_mut() {
                hs.retain(|h| !is_ours(spec, h));
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
        if let Some(l) = link {
            let mut entry = json!({"type": "command", "command": format!("{} {}", quote(l), spec.mark)});
            if spec.named {
                entry["name"] = json!("agentmux");
            }
            groups.push(json!({"hooks": [entry]}));
        }
        if groups.is_empty() {
            cfg["hooks"].as_object_mut().unwrap().remove(*ev);
        } else {
            cfg["hooks"][*ev] = json!(groups);
        }
    }
    if cfg["hooks"].as_object().is_some_and(|o| o.is_empty()) {
        cfg.as_object_mut().unwrap().remove("hooks");
    }
    atomic_write(path, &serde_json::to_string_pretty(&cfg)?)
}

pub fn hook_enabled(spec: &HookSpec, path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| {
            spec.events.iter().all(|ev| v["hooks"][*ev].as_array().is_some_and(|gs| gs.iter().any(|g| g["hooks"].as_array().is_some_and(|hs| hs.iter().any(|h| is_ours(spec, h))))))
        })
}

/// The hook's view of the current chat: which event, which chat, which folder.
#[derive(Default, Debug, Clone)]
pub struct HookEvent {
    pub event: String,
    pub chat: Option<String>,
    pub cwd: Option<String>,
}

impl HookEvent {
    pub fn parse(stdin: &str) -> HookEvent {
        let v: Value = serde_json::from_str(stdin).unwrap_or(Value::Null);
        HookEvent {
            event: v["hook_event_name"].as_str().unwrap_or("").to_string(),
            chat: v["session_id"].as_str().filter(|s| !s.is_empty()).map(String::from),
            cwd: v["cwd"].as_str().filter(|s| !s.is_empty()).map(String::from),
        }
    }
}

/// Run by the hook: remember which Claude chat is active in which folder (so questions it asks go
/// to that chat's AgentMux thread), then return what to add to Claude's context, if anything.
pub fn on_hook(spec: &HookSpec, vault: &crate::Vault, ev: &HookEvent) -> Result<Option<String>> {
    let mut lead = String::new();
    if let (Some(chat), Some(cwd)) = (&ev.chat, &ev.cwd) {
        let root = crate::context::workspace_root(Path::new(cwd));
        vault.touch_thread(spec.agent, chat, &root.to_string_lossy())?;
        // When a chat starts (or resumes), say which AgentMux thread it belongs to and where it stands.
        if ev.event == spec.start_event {
            if let Some(sess) = vault.thread_binding(spec.agent, chat)? {
                if let (Some(room), Some(title)) = (vault.session_room(&sess)?, vault.session_title(&sess)?) {
                    let room_name = vault.room(&room)?.and_then(|r| r["title"].as_str().map(String::from)).unwrap_or_default();
                    lead = format!("AgentMux: this chat is linked to the thread \"{room_name} › {title}\" (session {sess}). Questions you ask other agents from here continue it; mux_brief gives its full context.\n");
                    if let Some(sum) = crate::memory::summary_of(vault, &sess)? {
                        let state: String = sum.lines().skip_while(|l| !l.contains("Current state")).skip(1).take_while(|l| !l.starts_with("## ")).collect::<Vec<_>>().join("\n");
                        if !state.trim().is_empty() {
                            lead.push_str(&format!("Where it stands:\n{}\n", state.trim()));
                        }
                    }
                }
            }
        }
    }
    let updates = updates_text(vault, spec.agent)?;
    Ok(match (lead.is_empty(), updates) {
        (true, u) => u,
        (false, None) => Some(lead),
        (false, Some(u)) => Some(format!("{lead}\n{u}")),
    })
}

/// What's new for `agent` as text for its chat, or None when nothing is new. Advances the
/// agent's read cursor, so each update is told once.
pub fn updates_text(vault: &crate::Vault, agent: &str) -> Result<Option<String>> {
    let items = vault.updates_for(agent, true)?;
    if items.is_empty() {
        return Ok(None);
    }
    let clip = |s: &str, n: usize| if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() };
    let mut out = String::from("AgentMux: since your last message, other agents got back to you.\n");
    for u in &items {
        let who = u["withName"].as_str().unwrap_or("?");
        let t = u["ticket"].as_str().unwrap_or("");
        let q = clip(u["question"].as_str().unwrap_or(""), 120);
        let line = match (u["kind"].as_str(), u["status"].as_str()) {
            (Some("answer"), Some("answered")) => format!("- {who} answered your question \"{q}\" (ticket {t}): {}", clip(u["answer"].as_str().unwrap_or(""), 500)),
            (Some("answer"), _) => format!("- Your question to {who} \"{q}\" failed (ticket {t}): {}", clip(u["answer"].as_str().unwrap_or(""), 200)),
            (Some("notice"), _) => format!("- Notice: {}", clip(u["question"].as_str().unwrap_or(""), 300)),
            _ => format!("- {who} asked you (ticket {t}): \"{q}\". Answer it with mux_reply."),
        };
        out.push_str(&line);
        out.push('\n');
        if out.len() > 1500 {
            out.push_str("- …more: call mux_updates.\n");
            break;
        }
    }
    out.push_str("Mention these to the user if they are relevant to what they just asked.");
    Ok(Some(out))
}
