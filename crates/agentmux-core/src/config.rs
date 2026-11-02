use std::path::PathBuf;

/// Root of the local vault. `AGENTMUX_HOME` overrides it (used by tests).
pub fn home() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTMUX_HOME") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".agentmux")
}

pub fn db_path() -> PathBuf {
    home().join("vault.db")
}

pub fn threads_dir() -> PathBuf {
    home().join("threads")
}

pub fn run_dir() -> PathBuf {
    home().join("run")
}

pub fn socket_path() -> PathBuf {
    run_dir().join("agentmux.sock")
}

pub fn backups_dir() -> PathBuf {
    home().join("backups")
}

pub fn bin_dir() -> PathBuf {
    home().join("bin")
}

/// Stable path client configs point at; a symlink to the real binary.
pub fn bin_link() -> PathBuf {
    bin_dir().join("agentmux")
}

pub const MAX_HOPS: i64 = 3;
const DEFAULT_WAKE_TIMEOUT_SECS: u64 = 300;

/// How long a woken agent has to answer. `AGENTMUX_WAKE_TIMEOUT_SECS` overrides it (tests).
pub fn wake_timeout_secs() -> u64 {
    std::env::var("AGENTMUX_WAKE_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_WAKE_TIMEOUT_SECS)
}

/// Webhook-woken bots run a full agent turn (often with research), so they get longer.
pub fn webhook_timeout_secs() -> u64 {
    std::env::var("AGENTMUX_WAKE_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(900)
}

/// A new question to the same agent within this window continues the same session.
pub const SESSION_CONTINUE_MINUTES: i64 = 30;

pub const AGENTS: [(&str, &str); 4] = [
    ("cursor", "Cursor"),
    ("grok", "Grok Bot"),
    ("claude", "Claude Code"),
    ("chatgpt", "ChatGPT"),
];

pub fn is_known_agent(id: &str) -> bool {
    AGENTS.iter().any(|(a, _)| *a == id)
}

pub fn agent_name(id: &str) -> String {
    AGENTS
        .iter()
        .find(|(a, _)| *a == id)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| id.to_string())
}

/// `grok` (the original bot) or any `grok:<slug>` bot.
pub fn is_grok_family(id: &str) -> bool {
    id == "grok" || id.starts_with("grok:")
}

/// Lowercase a-z/0-9/- slug, at most 40 chars; empty if nothing usable is left.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    out.truncate(40);
    out.trim_end_matches('-').to_string()
}
