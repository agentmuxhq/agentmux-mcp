use crate::config;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

fn extra_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".local/bin"));
        v.push(h.join(".claude/local"));
        v.push(h.join(".cargo/bin"));
        v.push(h.join(".opencode/bin")); // OpenCode's install script
        // Global npm installs (gemini, codex, claude…) live wherever the user's Node manager puts
        // them; an app or login service doesn't inherit the shell's PATH, so look in each.
        for versions in [".nvm/versions/node", ".local/share/mise/installs/node", ".asdf/installs/nodejs", ".local/share/fnm/node-versions", "Library/Application Support/fnm/node-versions"] {
            if let Ok(rd) = std::fs::read_dir(h.join(versions)) {
                let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
                dirs.sort();
                dirs.reverse(); // newest version first
                for d in dirs {
                    v.push(d.join("bin"));
                    v.push(d.join("installation/bin")); // fnm layout
                }
            }
        }
        for d in [".local/share/mise/shims", ".asdf/shims", ".volta/bin", ".npm-global/bin", ".bun/bin", "Library/pnpm", ".yarn/bin"] {
            v.push(h.join(d));
        }
    }
    v.push("/opt/homebrew/bin".into());
    v.push("/usr/local/bin".into());
    v.push("/usr/bin".into());
    v.push("/bin".into());
    v
}

/// PATH for spawned CLIs. A GUI-launched app has a minimal PATH, so extend it.
pub fn spawn_path() -> String {
    let mut parts: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for d in extra_dirs() {
        let s = d.to_string_lossy().to_string();
        if !parts.contains(&s) {
            parts.push(s);
        }
    }
    parts.join(":")
}

pub fn find_bin(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    dirs.extend(extra_dirs());
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

const ENV_ALLOW: &[&str] = &[
    "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TMPDIR", "TERM", "CURSOR_API_KEY", "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY", "CODEX_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
    // Not GEMINI_API_KEY / GOOGLE_*: Gemini CLI keeps its own login, and a stale key exported in a
    // shell profile would override it (and an app started from that shell would pass it on).
];

/// Run the agent's CLI headlessly and return its final stdout as the reply.
pub async fn run_wake(
    agent: &str,
    prompt: &str,
    workspace: &str,
    allow_edits: bool,
    extra_env: &[(String, String)],
) -> Result<String> {
    let ticket = extra_env.iter().find(|(k, _)| k == "AGENTMUX_TICKET").map(|(_, v)| v.as_str()).unwrap_or("");
    let provider = crate::providers::get(agent).ok_or_else(|| anyhow!("unknown agent @{agent}"))?;
    let spec = provider
        .wake(&crate::providers::WakeRequest { prompt, workspace, allow_edits, ticket })
        .ok_or_else(|| anyhow!("no CLI found to wake @{agent}"))?;
    let bin = spec.bin;
    let mut cmd = Command::new(&bin);
    cmd.args(&spec.args)
        .current_dir(if std::path::Path::new(workspace).is_dir() { workspace } else { "." })
        .env_clear()
        .env("PATH", spawn_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for k in ENV_ALLOW {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().map_err(|e| anyhow!("failed to start {}: {e}", bin.display()))?;
    let out = tokio::time::timeout(Duration::from_secs(config::wake_timeout_secs()), child.wait_with_output())
        .await
        .map_err(|_| anyhow!("@{agent} did not answer within {}s", config::wake_timeout_secs()))??;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() || stdout.is_empty() {
        let detail = strip_ansi(if !stdout.is_empty() { &stdout } else { &stderr });
        let tail: String = detail.chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect();
        let hint = login_hint(agent, &bin.display().to_string(), &tail).map(|h| format!("\n\n{h}")).unwrap_or_default();
        return Err(anyhow!("@{agent} wake failed ({}): {}{hint}", out.status, tail));
    }
    Ok(stdout)
}

/// How to sign a CLI back in: (arguments to run it with, what the user does next). Keyed by the
/// binary's file name; None when AgentMux doesn't know that CLI.
fn login_steps(cli: &str) -> Option<(Vec<&'static str>, &'static str)> {
    Some(match cli {
        "claude" => (vec!["auth", "login"], "finish signing in in your browser"),
        "codex" => (vec!["login"], "finish signing in in your browser"),
        "gemini" => (vec![], "pick a sign-in method when it asks, and finish in your browser"),
        "cursor-agent" | "agent" => (vec!["login"], "finish signing in in your browser"),
        "opencode" => (vec!["auth", "login"], "pick your model provider and sign in"),
        _ => return None,
    })
}

fn cli_name(bin: &str) -> String {
    std::path::Path::new(bin).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
}

/// Does this wake output mean the agent's CLI is signed out?
pub fn is_signed_out(output: &str) -> bool {
    let o = output.to_lowercase();
    ["failed to authenticate", "oauth", "not logged in", "please log in", "please login", "/login", "invalid api key", "unauthenticated", "authentication required", "login required", "401"]
        .iter()
        .any(|k| o.contains(k))
}

/// A wake that failed because the agent's CLI is signed out: say how to sign back in, since only
/// the user can (it needs their browser). None for other failures.
pub fn login_hint(agent: &str, bin: &str, output: &str) -> Option<String> {
    if !is_signed_out(output) {
        return None;
    }
    let cli = cli_name(bin);
    let how = match login_steps(&cli) {
        Some((args, next)) => format!("open Terminal, run `{}`, then {next}", std::iter::once(cli.as_str()).chain(args).collect::<Vec<_>>().join(" ")),
        None => format!("sign in to `{cli}` again in Terminal"),
    };
    Some(format!("@{agent}'s command-line login has expired, so AgentMux can't wake it. To fix: {how}. Then send the test ping again. (In the app: Edit {agent} → Sign in.)"))
}

/// The command that signs `agent`'s CLI back in, for the app's "Sign in" button: (binary, args,
/// what the user does next in the Terminal window it opens).
pub fn login_command(agent: &str) -> Option<(std::path::PathBuf, Vec<String>, String)> {
    let p = crate::providers::get(agent)?;
    let spec = p.wake(&crate::providers::WakeRequest { prompt: "", workspace: "/", allow_edits: false, ticket: "" })?;
    let (args, next) = login_steps(&cli_name(&spec.bin.to_string_lossy()))?;
    Some((spec.bin, args.into_iter().map(String::from).collect(), next.to_string()))
}

/// Terminal colour codes (ESC [ … letter) out of a CLI's output, so errors read cleanly in the app.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}
