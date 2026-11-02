use crate::config;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const CODEX_BUNDLED: &str =
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex";

fn extra_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".local/bin"));
        v.push(h.join(".claude/local"));
        v.push(h.join(".cargo/bin"));
        if let Ok(rd) = std::fs::read_dir(h.join(".nvm/versions/node")) {
            for e in rd.flatten() {
                v.push(e.path().join("bin"));
            }
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

/// Resolve the executable used to wake (and configure) an agent.
pub fn agent_binary(agent: &str) -> Option<PathBuf> {
    match agent {
        "cursor" => find_bin("cursor-agent").or_else(|| find_bin("agent")),
        "claude" => find_bin("claude"),
        "chatgpt" => {
            let p = PathBuf::from(CODEX_BUNDLED);
            if p.is_file() { Some(p) } else { find_bin("codex") }
        }
        _ => None,
    }
}

pub fn build_args(agent: &str, prompt: &str, workspace: &str, allow_edits: bool) -> Option<Vec<String>> {
    let s = |x: &str| x.to_string();
    match agent {
        "cursor" => {
            let mut a = vec![s("-p"), prompt.to_string(), s("--trust"), s("--workspace"), s(workspace), s("--output-format"), s("text")];
            if !allow_edits {
                a.extend([s("--mode"), s("ask")]);
            }
            Some(a)
        }
        "claude" => Some(vec![
            s("-p"),
            prompt.to_string(),
            s("--permission-mode"),
            s(if allow_edits { "acceptEdits" } else { "plan" }),
        ]),
        "chatgpt" => Some(vec![
            s("exec"),
            s("-s"),
            s(if allow_edits { "workspace-write" } else { "read-only" }),
            s("--skip-git-repo-check"),
            s("--ephemeral"),
            s("-C"),
            s(workspace),
            prompt.to_string(),
        ]),
        _ => None,
    }
}

const ENV_ALLOW: &[&str] = &[
    "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TMPDIR", "TERM", "CURSOR_API_KEY", "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY", "CODEX_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
];

/// Run the agent's CLI headlessly and return its final stdout as the reply.
pub async fn run_wake(
    agent: &str,
    prompt: &str,
    workspace: &str,
    allow_edits: bool,
    extra_env: &[(String, String)],
) -> Result<String> {
    let bin = agent_binary(agent).ok_or_else(|| anyhow!("no CLI found to wake @{agent}"))?;
    let args = build_args(agent, prompt, workspace, allow_edits).ok_or_else(|| anyhow!("@{agent} has no CLI waker"))?;
    let mut cmd = Command::new(&bin);
    cmd.args(&args)
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
        let detail = if !stdout.is_empty() { stdout } else { stderr };
        let tail: String = detail.chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(anyhow!("@{agent} wake failed ({}): {}", out.status, tail));
    }
    Ok(stdout)
}
