use super::*;
use crate::wakers::{find_bin, spawn_path};
use std::process::Command;

/// ChatGPT / Codex. The id stays `chatgpt` so existing vaults and configs keep working.
pub struct Codex;

const BUNDLED: &str = "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex";

fn binary() -> Option<PathBuf> {
    let p = PathBuf::from(BUNDLED);
    if p.is_file() { Some(p) } else { find_bin("codex") }
}

fn run(args: &[&str]) -> Result<String> {
    let bin = binary().ok_or_else(|| anyhow::anyhow!("chatgpt CLI not found"))?;
    let out = Command::new(&bin).args(args).env("PATH", spawn_path()).output()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("{} {} failed: {}", bin.display(), args.join(" "), text.trim());
    }
    Ok(text.trim().to_string())
}

impl Provider for Codex {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "chatgpt".into(), name: "ChatGPT".into(), color: "#8B5CF6".into(), hint: "MCP stdio · ~/.codex/config.toml".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        match binary() {
            Some(b) => (true, b.display().to_string()),
            None => (false, "chatgpt CLI not found".into()),
        }
    }
    fn configured(&self) -> bool {
        run(&["mcp", "list"]).map(|s| s.contains("agentmux")).unwrap_or(false)
    }
    fn install(&self, link: &Path) -> Result<String> {
        let l = link.to_string_lossy().to_string();
        let _ = run(&["mcp", "remove", "agentmux"]);
        run(&["mcp", "add", "agentmux", "--", &l, "mcp", "--agent", "chatgpt"])?;
        Ok("Registered 'agentmux' with ChatGPT/Codex (~/.codex/config.toml). Restart the ChatGPT app to load it.".into())
    }
    fn uninstall(&self) -> Result<String> {
        run(&["mcp", "remove", "agentmux"])
    }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let s = |x: &str| x.to_string();
        Some(WakeSpec {
            bin: binary()?,
            args: vec![
                s("exec"), s("-s"), s(if r.allow_edits { "workspace-write" } else { "read-only" }),
                s("--skip-git-repo-check"), s("--ephemeral"), s("-C"), s(r.workspace), s(r.prompt),
            ],
        })
    }
}
