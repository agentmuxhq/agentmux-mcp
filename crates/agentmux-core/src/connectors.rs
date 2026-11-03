use crate::config;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

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

pub fn detect(agent: &str) -> Value {
    match crate::providers::get(agent) {
        Some(p) => crate::providers::describe(p.as_ref()),
        None => json!({"agent": agent, "installed": false, "detail": "unknown agent", "configured": false}),
    }
}

/// Register AgentMux as an MCP server in the agent. Never prints existing config contents.
pub fn install(agent: &str) -> Result<String> {
    let p = crate::providers::get(agent).ok_or_else(|| anyhow!("unknown agent '{agent}'"))?;
    let link = ensure_bin_link()?;
    p.install(&link)
}

pub fn uninstall(agent: &str) -> Result<String> {
    let p = crate::providers::get(agent).ok_or_else(|| anyhow!("unknown agent '{agent}'"))?;
    p.uninstall()
}

/// A provider-specific switch, e.g. Claude Code's `hook`.
pub fn set_option(agent: &str, name: &str, on: bool) -> Result<String> {
    crate::providers::get(agent).ok_or_else(|| anyhow!("unknown agent '{agent}'"))?.set_option(name, on)
}

/// Once per provider, switch on the context hook for agents that were connected before hooks
/// existed. Remembered in the vault, so a hook the user turned off is never re-added.
pub fn adopt_context_hooks(vault: &crate::Vault) {
    for p in crate::providers::all().into_iter().filter(|p| p.caps().context_hook) {
        let key = format!("hook.{}.adopted", p.info().id);
        if vault.setting(&key).ok().flatten().is_some() || !p.configured() {
            continue;
        }
        match p.set_option("hook", true) {
            Ok(_) => tracing::info!("context hook enabled for {}", p.info().id),
            Err(e) => tracing::warn!("could not enable the context hook for {}: {e}", p.info().id),
        }
        let _ = vault.set_setting(&key, "1");
    }
}
