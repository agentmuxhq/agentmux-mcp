use crate::core::{CallerCtx, Core};
use crate::gateway::Gateway;
use crate::{config, mcp};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

/// Bind the per-user Unix socket (dir 0700, socket 0600) and serve shim connections until killed.
/// This is the headless entry point (`agentmux daemon`); it refuses to start while AgentMux is turned off.
pub async fn serve(core: Arc<Core>) -> Result<()> {
    if config::is_paused() {
        bail!("AgentMux is turned off. Turn it on from the AgentMux menu bar icon first.");
    }
    let (_keep_open, rx) = watch::channel(false);
    serve_until(core, rx).await
}

/// Like `serve`, but stops (closing every connection and removing the socket) once `shutdown` flips to true.
pub async fn serve_until(core: Arc<Core>, shutdown: watch::Receiver<bool>) -> Result<()> {
    serve_with(core, None, shutdown).await
}

/// `serve_until` plus a control channel for the tunnel (used by `agentmux tunnel …`) when this
/// process also runs the HTTP gateway.
pub async fn serve_with(core: Arc<Core>, gateway: Option<Arc<Gateway>>, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    let dir = config::run_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let sock = config::socket_path();
    // sockaddr_un.sun_path is 104 bytes on macOS (108 on Linux), including the NUL.
    if sock.as_os_str().len() > 100 {
        bail!(
            "socket path is too long ({} bytes, max ~100): {}\nSet AGENTMUX_HOME to a shorter directory.",
            sock.as_os_str().len(),
            sock.display()
        );
    }
    if sock.exists() {
        if UnixStream::connect(&sock).await.is_ok() {
            bail!("agentmux daemon already running ({})", sock.display());
        }
        std::fs::remove_file(&sock)?;
    }
    let listener = UnixListener::bind(&sock)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!("agentmux daemon listening on {}", sock.display());
    loop {
        let (stream, _) = tokio::select! {
            r = listener.accept() => r?,
            _ = shutdown.changed() => break,
        };
        let (core, rx, gw) = (core.clone(), shutdown.clone(), gateway.clone());
        tokio::spawn(async move {
            if let Err(e) = handle_conn(core, gw, stream, rx).await {
                tracing::debug!("connection ended: {e}");
            }
        });
    }
    drop(listener);
    let _ = std::fs::remove_file(&sock);
    Ok(())
}

/// The daemon embedded in the desktop app, with an on/off switch (the menu bar toggle).
pub struct Switch {
    core: Arc<Core>,
    gateway: Option<Arc<Gateway>>,
    running: std::sync::Mutex<Option<(watch::Sender<bool>, tokio::task::JoinHandle<Result<()>>)>>,
}

impl Switch {
    pub fn new(core: Arc<Core>) -> Arc<Self> {
        Arc::new(Self { core, gateway: None, running: std::sync::Mutex::new(None) })
    }

    /// Also answer tunnel control requests (`agentmux tunnel …`) for this gateway.
    pub fn with_gateway(core: Arc<Core>, gateway: Arc<Gateway>) -> Arc<Self> {
        Arc::new(Self { core, gateway: Some(gateway), running: std::sync::Mutex::new(None) })
    }

    pub fn is_on(&self) -> bool {
        self.running.lock().unwrap().as_ref().is_some_and(|(_, h)| !h.is_finished())
    }

    /// Turn on. Errors if the socket can't be bound (for example another daemon already owns it).
    pub async fn start(&self) -> Result<()> {
        if self.is_on() {
            return Ok(());
        }
        let _ = std::fs::remove_file(config::paused_marker());
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(serve_with(self.core.clone(), self.gateway.clone(), rx));
        *self.running.lock().unwrap() = Some((tx, handle));
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let finished = self.running.lock().unwrap().as_ref().is_some_and(|(_, h)| h.is_finished());
            if finished {
                let (_, h) = self.running.lock().unwrap().take().unwrap();
                return match h.await {
                    Ok(Err(e)) => Err(e),
                    Ok(Ok(())) => bail!("the daemon stopped right after starting"),
                    Err(e) => bail!("the daemon crashed: {e}"),
                };
            }
            if config::socket_path().exists() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Turn off: nobody can connect, open connections close, and the marker keeps shims from respawning it.
    pub async fn stop(&self) {
        let _ = std::fs::create_dir_all(config::run_dir());
        let _ = std::fs::write(config::paused_marker(), "off\n");
        let taken = self.running.lock().unwrap().take();
        if let Some((tx, handle)) = taken {
            let _ = tx.send(true);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
        }
        let _ = std::fs::remove_file(config::socket_path());
    }
}

async fn handle_conn(core: Arc<Core>, gateway: Option<Arc<Gateway>>, stream: UnixStream, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();

    // First line identifies the caller: {"agentmux_hello":{"agent","cwd","hop"}}
    let Some(first) = (tokio::select! { l = lines.next_line() => l?, _ = shutdown.changed() => return Ok(()) }) else { return Ok(()) };
    let hello: Value = serde_json::from_str(&first)?;
    if hello.get("agentmux_control").is_some() {
        return control(gateway, lines, w).await;
    }
    let h = &hello["agentmux_hello"];
    let agent = h["agent"].as_str().unwrap_or("").to_string();
    if !(core.vault.is_agent(&agent) || agent == "user") {
        w.write_all(format!("{}\n", json!({"error": format!("unknown agent '{agent}'")})).as_bytes()).await?;
        bail!("unknown agent");
    }
    let ctx = CallerCtx {
        agent,
        cwd: h["cwd"].as_str().unwrap_or(".").to_string(),
        hop: h["hop"].as_i64().unwrap_or(0).clamp(0, config::MAX_HOPS),
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if w.write_all(line.as_bytes()).await.is_err() || w.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    // Requests run concurrently so a long mux_ask_peer doesn't block pings.
    loop {
        let line = tokio::select! {
            l = lines.next_line() => match l? { Some(l) => l, None => break },
            _ = shutdown.changed() => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        let (core, ctx, tx) = (core.clone(), ctx.clone(), tx.clone());
        tokio::spawn(async move {
            if let Some(resp) = mcp::handle(&core, &ctx, req).await {
                let _ = tx.send(resp.to_string());
            }
        });
    }
    drop(tx);
    let _ = writer.await;
    Ok(())
}

/// Send one control command to whichever AgentMux process owns the socket.
pub async fn control_request(cmd: &str) -> Result<Value> {
    let stream = UnixStream::connect(config::socket_path()).await.map_err(|_| anyhow::anyhow!("AgentMux isn't running (agentmux start)"))?;
    let (r, mut w) = stream.into_split();
    w.write_all(format!("{}\n{}\n", json!({"agentmux_control": true}), json!({"cmd": cmd})).as_bytes()).await?;
    let mut lines = BufReader::new(r).lines();
    let line = lines.next_line().await?.ok_or_else(|| anyhow::anyhow!("no answer from AgentMux (it may be an older version)"))?;
    let v: Value = serde_json::from_str(&line)?;
    if v["ok"] == true {
        Ok(v["result"].clone())
    } else {
        bail!("{}", v["error"].as_str().unwrap_or("failed"))
    }
}

/// One request per connection: `{"cmd": "tunnel.status" | "tunnel.start" | "tunnel.stop" | "tunnel.verify" | "tunnel.restore" | "tunnel.dismiss_lost" | "tokens.reload" | "ping"}`.
/// Only reachable through the 0600 socket, i.e. by this macOS user.
async fn control(
    gateway: Option<Arc<Gateway>>,
    mut lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    mut w: tokio::net::unix::OwnedWriteHalf,
) -> Result<()> {
    let Some(line) = lines.next_line().await? else { return Ok(()) };
    let req: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    let cmd = req["cmd"].as_str().unwrap_or("");
    let out = match (cmd, &gateway) {
        ("ping", g) => json!({"ok": true, "result": {"gateway": g.is_some()}}),
        (_, None) => json!({"ok": false, "error": "this AgentMux process doesn't run the tunnel"}),
        ("tunnel.status", Some(g)) => json!({"ok": true, "result": g.status()}),
        ("tunnel.start", Some(g)) => match g.start().await {
            Ok(()) => json!({"ok": true, "result": g.status()}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        },
        ("tunnel.stop", Some(g)) => {
            g.stop().await;
            json!({"ok": true, "result": g.status()})
        }
        ("tunnel.restore", Some(g)) => match g.restore_last_good() {
            Ok(h) => json!({"ok": true, "result": h}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        },
        ("tunnel.dismiss_lost", Some(g)) => match g.dismiss_lost() {
            Ok(()) => json!({"ok": true, "result": g.status()}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        },
        // Another process rotated or removed a bot token in the Keychain: drop cached ones.
        ("tokens.reload", Some(g)) => {
            g.reload_tokens();
            json!({"ok": true, "result": true})
        }
        ("tunnel.verify", Some(g)) => match g.verify().await {
            Ok(m) => json!({"ok": true, "result": m}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        },
        _ => json!({"ok": false, "error": format!("unknown command '{cmd}'")}),
    };
    w.write_all(format!("{out}\n").as_bytes()).await?;
    let _ = w.shutdown().await;
    Ok(())
}
