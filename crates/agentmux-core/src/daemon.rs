use crate::core::{CallerCtx, Core};
use crate::{config, mcp};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// Bind the per-user Unix socket (dir 0700, socket 0600) and serve shim connections.
pub async fn serve(core: Arc<Core>) -> Result<()> {
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
        let (stream, _) = listener.accept().await?;
        let core = core.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(core, stream).await {
                tracing::debug!("connection ended: {e}");
            }
        });
    }
}

async fn handle_conn(core: Arc<Core>, stream: UnixStream) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();

    // First line identifies the caller: {"agentmux_hello":{"agent","cwd","hop"}}
    let Some(first) = lines.next_line().await? else { return Ok(()) };
    let hello: Value = serde_json::from_str(&first)?;
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
    while let Some(line) = lines.next_line().await? {
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
