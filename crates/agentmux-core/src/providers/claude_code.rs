use super::*;
use crate::wakers::{find_bin, spawn_path};
use std::process::Command;

/// Claude Code. Besides the MCP server it installs a context hook (see `hook.rs` in the CLI) that
/// tells Claude, each time the user sends a message, what other agents have answered meanwhile.
pub struct ClaudeCode;

fn binary() -> Option<PathBuf> {
    find_bin("claude")
}

fn run(args: &[&str]) -> Result<String> {
    let bin = binary().ok_or_else(|| anyhow::anyhow!("claude CLI not found"))?;
    let out = Command::new(&bin).args(args).env("PATH", spawn_path()).output()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("{} {} failed: {}", bin.display(), args.join(" "), text.trim());
    }
    Ok(text.trim().to_string())
}

/// `~/.claude/settings.json`; `AGENTMUX_CLAUDE_SETTINGS` overrides it (tests).
pub fn settings_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTMUX_CLAUDE_SETTINGS") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".claude/settings.json")
}

const EVENTS: [&str; 2] = ["UserPromptSubmit", "SessionStart"];
const MARK: &str = "hook claude-code";

fn is_ours(h: &Value) -> bool {
    h["command"].as_str().is_some_and(|c| c.contains(".agentmux") || c.contains("agentmux")) && h["command"].as_str().is_some_and(|c| c.ends_with(MARK))
}

fn quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.contains(' ') { format!("'{s}'") } else { s.to_string() }
}

/// Add (or, with `link = None`, remove) our hook entries, leaving everything else alone.
pub fn edit_hooks(path: &Path, link: Option<&Path>) -> Result<()> {
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
    for ev in EVENTS {
        let mut groups: Vec<Value> = cfg["hooks"][ev].as_array().cloned().unwrap_or_default();
        // Drop our old entries first so a re-install never duplicates.
        for g in groups.iter_mut() {
            if let Some(hs) = g["hooks"].as_array_mut() {
                hs.retain(|h| !is_ours(h));
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
        if let Some(l) = link {
            groups.push(json!({"hooks": [{"type": "command", "command": format!("{} {MARK}", quote(l))}]}));
        }
        if groups.is_empty() {
            cfg["hooks"].as_object_mut().unwrap().remove(ev);
        } else {
            cfg["hooks"][ev] = json!(groups);
        }
    }
    if cfg["hooks"].as_object().is_some_and(|o| o.is_empty()) {
        cfg.as_object_mut().unwrap().remove("hooks");
    }
    atomic_write(path, &serde_json::to_string_pretty(&cfg)?)
}

pub fn hook_enabled() -> bool {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| EVENTS.iter().all(|ev| v["hooks"][ev].as_array().is_some_and(|gs| gs.iter().any(|g| g["hooks"].as_array().is_some_and(|hs| hs.iter().any(is_ours))))))
}

impl Provider for ClaudeCode {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "claude".into(), name: "Claude Code".into(), color: "#10B981".into(), hint: "MCP stdio · claude mcp add".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: true }
    }
    fn detect(&self) -> (bool, String) {
        match binary() {
            Some(b) => (true, b.display().to_string()),
            None => (false, "claude CLI not found".into()),
        }
    }
    fn configured(&self) -> bool {
        run(&["mcp", "list"]).map(|s| s.contains("agentmux")).unwrap_or(false)
    }
    fn install(&self, link: &Path) -> Result<String> {
        let l = link.to_string_lossy().to_string();
        let _ = run(&["mcp", "remove", "agentmux", "--scope", "user"]);
        run(&["mcp", "add", "--scope", "user", "agentmux", "--", &l, "mcp", "--agent", "claude"])?;
        let hook = match edit_hooks(&settings_path(), Some(link)) {
            Ok(()) => " The context hook is on: Claude is told what's new each time you send a message.",
            Err(_) => " (Could not add the context hook to ~/.claude/settings.json.)",
        };
        Ok(format!("Registered 'agentmux' with Claude Code (user scope). New Claude sessions will see it.{hook}"))
    }
    fn uninstall(&self) -> Result<String> {
        let _ = edit_hooks(&settings_path(), None);
        run(&["mcp", "remove", "agentmux", "--scope", "user"])
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let s = |x: &str| x.to_string();
        Some(WakeSpec {
            bin: binary()?,
            args: vec![s("-p"), s(r.prompt), s("--permission-mode"), s(if r.allow_edits { "acceptEdits" } else { "plan" })],
        })
    }
    fn set_option(&self, name: &str, on: bool) -> Result<String> {
        if name != "hook" {
            bail!("Claude Code has no option '{name}'");
        }
        if on {
            edit_hooks(&settings_path(), Some(&crate::config::bin_link()))?;
            Ok("Claude Code will be told what's new each time you send a message.".into())
        } else {
            edit_hooks(&settings_path(), None)?;
            Ok("Context hook removed from ~/.claude/settings.json.".into())
        }
    }
    fn options(&self) -> Value {
        json!({"hook": hook_enabled()})
    }
}

/// The text the hook prints into Claude Code's context, or None when nothing is new.
/// Advances Claude's read cursor, so each update is told once.
pub fn hook_context(vault: &crate::Vault) -> Result<Option<String>> {
    let items = vault.updates_for("claude", true)?;
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
