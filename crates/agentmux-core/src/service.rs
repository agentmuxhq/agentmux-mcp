//! The headless AgentMux (`agentmux daemon`, run at login by `agentmux init`): everything the
//! desktop app does in the background, without the window. Either one can own the socket and the
//! HTTP port; whichever starts second waits and takes over when the other goes away.

use crate::core::Core;
use crate::gateway::Gateway;
use crate::{config, daemon, sync};
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

pub async fn run_headless(core: Arc<Core>) -> Result<()> {
    if config::is_paused() {
        // Turned off from the menu bar or with `agentmux stop`: exit cleanly so launchd leaves it off.
        tracing::info!("AgentMux is turned off; not starting");
        return Ok(());
    }
    let gw = Gateway::new(core.clone());

    // HTTP gateway (Grok bots): served only while this process owns the socket, so the tunnel and
    // the port always live in the same process (the desktop app takes both when it is the owner).
    let owns = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let o = owns.clone();
    tokio::spawn(gw.clone().serve_http_while(move || o.load(std::sync::atomic::Ordering::SeqCst)));
    let o = owns.clone();
    tokio::spawn(core.clone().memory_worker(move || o.load(std::sync::atomic::Ordering::SeqCst)));

    // Tunnel: once the HTTP endpoint is up, take over a running tunnel or start the permanent one.
    let g = gw.clone();
    let v0 = core.vault.clone();
    tokio::spawn(async move {
        while !g.http_bound() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        // Brought back after an update restart if it was running before, even with autostart off.
        let resume = v0.setting("tunnel.resume_after_update").ok().flatten().is_some();
        let _ = v0.del_setting("tunnel.resume_after_update");
        if !g.adopt().await && (g.autostart() || resume) && g.permanent() {
            if let Err(e) = g.start().await {
                tracing::warn!("could not start the tunnel: {e}");
            }
        }
    });

    // Agents connected before context hooks existed get theirs once.
    let v = core.vault.clone();
    tokio::task::spawn_blocking(move || crate::connectors::adopt_context_hooks(&v));

    // Close orphaned questions; auto-sync when switched on.
    let c = core.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            c.sweep_stale();
        }
    });
    let c = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            let on = c.vault.setting("sync.auto").ok().flatten().as_deref() == Some("1") && c.vault.setting("sync.remote").ok().flatten().is_some();
            if on && !config::is_paused() {
                if let Err(e) = sync::sync_now(&c.vault).await {
                    tracing::warn!("auto sync failed: {e}");
                }
            }
            tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        }
    });

    // Updated in place (the app update replaces the binary): restart onto the new version, or the
    // service keeps running old code until the next login. Only under launchd, which brings it back.
    if std::env::var("XPC_SERVICE_NAME").as_deref() == Ok("dev.agentmux.daemon") {
        let c = core.clone();
        let g = gw.clone();
        tokio::spawn(async move {
            let Some(exe) = std::env::current_exe().ok() else { return };
            let stamp = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let started = stamp(&exe);
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let now = stamp(&exe);
                if now.is_none() || now == started {
                    continue; // mid-replace, or unchanged
                }
                // Let running wakes finish (up to 10 minutes); questions waiting in inboxes are kept.
                for _ in 0..60 {
                    let busy = c.vault.open_tickets().map(|t| t.iter().any(|x| x["status"] == "running")).unwrap_or(false);
                    if !busy {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                // The tunnel goes down with this process: bring it back once on the new version.
                if g.status()["state"] == "running" {
                    let _ = c.vault.set_setting("tunnel.resume_after_update", "1");
                }
                tracing::info!("AgentMux was updated; restarting the service on the new version");
                std::process::exit(75); // non-zero: launchd's KeepAlive starts the new binary
            }
        });
    }

    // Turned off (`agentmux stop`, or the app's menu bar): take the tunnel down and exit cleanly.
    let (tx, rx) = watch::channel(false);
    let g = gw.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if config::is_paused() {
                g.stop().await;
                let _ = tx.send(true);
                break;
            }
        }
    });

    // The socket. If the desktop app (or another daemon) has it, wait and take over when it's gone.
    let mut waiting = false;
    loop {
        if config::is_paused() {
            gw.stop().await;
            return Ok(());
        }
        owns.store(true, std::sync::atomic::Ordering::SeqCst);
        let served = daemon::serve_with(core.clone(), Some(gw.clone()), rx.clone()).await;
        owns.store(false, std::sync::atomic::Ordering::SeqCst);
        match served {
            Ok(()) => {
                gw.stop().await;
                return Ok(());
            }
            Err(e) if e.to_string().contains("already running") => {
                if !waiting {
                    tracing::info!("another AgentMux owns the socket; standing by");
                    waiting = true;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            Err(e) => return Err(e),
        }
    }
}
