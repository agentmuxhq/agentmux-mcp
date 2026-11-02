//! The login service that keeps the headless daemon running (`agentmux init`, `start`, `stop`).
//! A LaunchAgent in the user's GUI session, so it can use the Keychain like the app does.

use agentmux_core::config;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LABEL: &str = "dev.agentmux.daemon";

pub fn plist_path() -> PathBuf {
    dirs_home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The LaunchAgent definition. Restarted if it crashes, but not after a clean exit (which is
/// what the daemon does when AgentMux has been turned off).
pub fn plist(bin: &Path, log: &Path) -> String {
    let home = dirs_home();
    let path = format!(
        "{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        home.display()
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key>
  <dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Background</string>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>{path}</string></dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        bin = xml_escape(&bin.to_string_lossy()),
        log = xml_escape(&log.to_string_lossy()),
        path = xml_escape(&path),
    )
}

fn uid() -> Result<String> {
    let out = Command::new("id").arg("-u").output().context("running id -u")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn launchctl(args: &[&str]) -> Result<(bool, String)> {
    let out = Command::new("launchctl").args(args).output().context("running launchctl")?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

pub fn loaded() -> bool {
    uid().and_then(|u| launchctl(&["print", &format!("gui/{u}/{LABEL}")])).map(|(ok, _)| ok).unwrap_or(false)
}

pub fn installed() -> bool {
    plist_path().exists()
}

/// Write the LaunchAgent and (re)load it.
pub fn install(bin: &Path) -> Result<()> {
    let logs = config::home().join("logs");
    std::fs::create_dir_all(&logs)?;
    let p = plist_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    agentmux_core::vault::atomic_write(&p, &plist(bin, &logs.join("daemon.log")))?;
    let u = uid()?;
    if loaded() {
        let _ = launchctl(&["bootout", &format!("gui/{u}/{LABEL}")]);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let (ok, text) = launchctl(&["bootstrap", &format!("gui/{u}"), &p.to_string_lossy()])?;
    if !ok {
        bail!("launchctl bootstrap failed: {}", text.trim());
    }
    Ok(())
}

pub fn start() -> Result<()> {
    if !installed() {
        bail!("the login service isn't installed; run `agentmux init` first");
    }
    let u = uid()?;
    let (ok, text) = if loaded() {
        launchctl(&["kickstart", "-k", &format!("gui/{u}/{LABEL}")])?
    } else {
        launchctl(&["bootstrap", &format!("gui/{u}"), &plist_path().to_string_lossy()])?
    };
    if !ok {
        bail!("launchctl failed: {}", text.trim());
    }
    Ok(())
}

pub fn stop() -> Result<()> {
    if loaded() {
        let u = uid()?;
        let (ok, text) = launchctl(&["bootout", &format!("gui/{u}/{LABEL}")])?;
        if !ok {
            bail!("launchctl bootout failed: {}", text.trim());
        }
    }
    Ok(())
}

pub fn uninstall() -> Result<()> {
    stop()?;
    let p = plist_path();
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_is_valid_and_points_at_the_daemon() {
        let dir = std::env::temp_dir().join(format!("amx-plist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dev.agentmux.daemon.plist");
        let text = plist(Path::new("/Users/me/.agentmux/bin/agentmux"), Path::new("/Users/me/.agentmux/logs/daemon.log"));
        std::fs::write(&file, &text).unwrap();
        // Apple's own validator.
        let lint = Command::new("plutil").arg("-lint").arg(&file).output().unwrap();
        assert!(lint.status.success(), "{}", String::from_utf8_lossy(&lint.stdout));
        assert!(text.contains("<string>/Users/me/.agentmux/bin/agentmux</string>\n    <string>daemon</string>"));
        assert!(text.contains("<key>SuccessfulExit</key><false/>"), "a clean exit (turned off) must not be restarted");
        let _ = std::fs::remove_dir_all(dir);
    }
}
