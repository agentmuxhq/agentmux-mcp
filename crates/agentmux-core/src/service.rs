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

    // HTTP gateway (Grok bots). The desktop app may hold the port; retry until it's free.
    let g = gw.clone();
    tokio::spawn(async move {
        let mut warned = false;
        loop {
            match g.clone().serve_http().await {
                Ok(()) => break,
                Err(e) => {
                    if !warned {
                        tracing::info!("HTTP gateway waiting: {e}");
                        warned = true;
                    }
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        }
    });

    // Tunnel: once the HTTP endpoint is up, take over a running tunnel or start the permanent one.
    let g = gw.clone();
    tokio::spawn(async move {
        while !g.http_bound() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if !g.adopt().await && g.autostart() && g.permanent() {
            if let Err(e) = g.start().await {
                tracing::warn!("could not start the tunnel: {e}");
            }
        }
    });

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
        match daemon::serve_with(core.clone(), Some(gw.clone()), rx.clone()).await {
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
