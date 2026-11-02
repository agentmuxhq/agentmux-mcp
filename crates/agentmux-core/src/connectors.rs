use crate::{config, vault::atomic_write, wakers};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Point `~/.agentmux/bin/agentmux` at the running binary so client configs survive rebuilds/updates.
/// Where `~/.agentmux/bin/agentmux` should point for a binary running at `exe`, and whether the
/// binary has to be copied there first because its own location won't survive.
///  * npx runs from a cache that gets cleaned, so the binary is copied into ~/.agentmux/bin.
///  * Homebrew runs from a versioned Cellar path that `brew upgrade` deletes; use the stable
///    `<prefix>/bin/agentmux` instead.
///  * The app bundle and a dev build are linked as they are.
pub fn stable_target(exe: &Path) -> (PathBuf, bool) {
    let s = exe.to_string_lossy();
    if s.contains("/_npx/") || s.contains("/node_modules/") {
        return (config::bin_dir().join("agentmux-cli"), true);
    }
    if let Some(i) = s.find("/Cellar/agentmux/") {
        let brew_bin = PathBuf::from(&s[..i]).join("bin").join("agentmux");
        return (brew_bin, false);
    }
    (exe.to_path_buf(), false)
}

pub fn ensure_bin_link() -> Result<PathBuf> {
    let mut exe = std::env::current_exe()?;
    // Inside the desktop app, current_exe() is the GUI; the stdio shim is the sibling `agentmux` CLI.
    let sibling = exe.with_file_name("agentmux");
    if sibling.is_file() {
        exe = sibling;
    } else if exe.file_name().map(|n| n != "agentmux").unwrap_or(true) {
        bail!("agentmux CLI binary not found next to {} (build it with: cargo build -p agentmux-cli)", exe.display());
    }
    let link = config::bin_link();
    std::fs::create_dir_all(config::bin_dir())?;
    let (target, copy) = stable_target(&exe);
    if copy {
        // Copy then rename, so a running daemon using the old file is never left with a half-written one.
        let tmp = target.with_extension("tmp");
        std::fs::copy(&exe, &tmp).with_context(|| format!("copy {} to {}", exe.display(), tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, &target)?;
    }
    let exe = target;
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(&link)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&exe, &link).with_context(|| format!("symlink {}", link.display()))?;
    Ok(link)
}

fn cursor_config() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".cursor/mcp.json")
}

pub fn detect(agent: &str) -> Value {
    let (installed, detail) = match agent {
        "cursor" => {
            let bin = wakers::agent_binary("cursor");
            let cfg = cursor_config().parent().map(|p| p.exists()).unwrap_or(false);
            (cfg || bin.is_some(), bin.map(|b| b.display().to_string()).unwrap_or_else(|| "Cursor app found; cursor-agent CLI missing (needed to wake it)".into()))
        }
        "claude" | "chatgpt" => match wakers::agent_binary(agent) {
            Some(b) => (true, b.display().to_string()),
            None => (false, format!("{agent} CLI not found")),
        },
        "grok" => (
            std::path::Path::new("/Applications/Grok Bot.app").exists(),
            "Grok Bot app (add AgentMux in its MCP settings)".into(),
        ),
        _ => (false, "unknown agent".into()),
    };
    json!({"agent": agent, "installed": installed, "detail": detail, "configured": is_configured(agent)})
}

fn is_configured(agent: &str) -> bool {
    match agent {
        "cursor" => std::fs::read_to_string(cursor_config())
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| v["mcpServers"]["agentmux"].is_object())
            .unwrap_or(false),
        "claude" => cli_has_server(agent),
        "chatgpt" => cli_has_server(agent),
        _ => false,
    }
}

fn cli_has_server(agent: &str) -> bool {
    let Some(bin) = wakers::agent_binary(agent) else { return false };
    Command::new(bin)
        .args(["mcp", "list"])
        .env("PATH", wakers::spawn_path())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("agentmux"))
        .unwrap_or(false)
}

fn run_cli(agent: &str, args: &[&str]) -> Result<String> {
    let bin = wakers::agent_binary(agent).ok_or_else(|| anyhow!("{agent} CLI not found"))?;
    let out = Command::new(&bin).args(args).env("PATH", wakers::spawn_path()).output()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("{} {} failed: {}", bin.display(), args.join(" "), text.trim());
    }
    Ok(text.trim().to_string())
}

/// Register AgentMux as an MCP server in the agent. Never prints existing config contents.
pub fn install(agent: &str) -> Result<String> {
    let link = ensure_bin_link()?;
    let l = link.to_string_lossy().to_string();
    match agent {
        "cursor" => {
            let path = cursor_config();
            let mut cfg: Value = if path.exists() {
                let raw = std::fs::read_to_string(&path)?;
                let bak = path.with_extension("json.agentmux.bak");
                if !bak.exists() {
                    atomic_write(&bak, &raw)?; // 0600; original may contain tokens
                }
                serde_json::from_str(&raw).context("~/.cursor/mcp.json is not valid JSON; refusing to edit")?
            } else {
                std::fs::create_dir_all(path.parent().unwrap())?;
                json!({})
            };
            if !cfg["mcpServers"].is_object() {
                cfg["mcpServers"] = json!({});
            }
            cfg["mcpServers"]["agentmux"] = json!({"command": l, "args": ["mcp", "--agent", "cursor"]});
            atomic_write(&path, &serde_json::to_string_pretty(&cfg)?)?;
            Ok(format!("Added 'agentmux' to {} (backup: mcp.json.agentmux.bak). Restart Cursor to load it.", path.display()))
        }
        "claude" => {
            let _ = run_cli("claude", &["mcp", "remove", "agentmux", "--scope", "user"]);
            run_cli("claude", &["mcp", "add", "--scope", "user", "agentmux", "--", &l, "mcp", "--agent", "claude"])?;
            Ok("Registered 'agentmux' with Claude Code (user scope). New Claude sessions will see it.".into())
        }
        "chatgpt" => {
            let _ = run_cli("chatgpt", &["mcp", "remove", "agentmux"]);
            run_cli("chatgpt", &["mcp", "add", "agentmux", "--", &l, "mcp", "--agent", "chatgpt"])?;
            Ok("Registered 'agentmux' with ChatGPT/Codex (~/.codex/config.toml). Restart the ChatGPT app to load it.".into())
        }
        "grok" => Ok(format!(
            "Grok Bot keeps MCP servers in its own settings, so add this by hand:\n  Grok Bot → Settings → MCP → Add server (stdio)\n  Command: {l}\n  Args:    mcp --agent grok"
        )),
        _ => bail!("unknown agent '{agent}'"),
    }
}

pub fn uninstall(agent: &str) -> Result<String> {
    match agent {
        "cursor" => {
            let path = cursor_config();
            let mut cfg: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
            if let Some(m) = cfg["mcpServers"].as_object_mut() {
                m.remove("agentmux");
            }
            atomic_write(&path, &serde_json::to_string_pretty(&cfg)?)?;
            Ok("Removed 'agentmux' from ~/.cursor/mcp.json".into())
        }
        "claude" => run_cli("claude", &["mcp", "remove", "agentmux", "--scope", "user"]),
        "chatgpt" => run_cli("chatgpt", &["mcp", "remove", "agentmux"]),
        _ => Ok("Remove the AgentMux server in that app's MCP settings.".into()),
    }
}
