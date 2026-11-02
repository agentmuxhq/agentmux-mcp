//! The menu-bar on/off switch for the embedded daemon.

use agentmux_core::{config, daemon, Core, Vault};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::test]
async fn daemon_can_be_turned_off_and_on() {
    // The socket path has a ~100 byte limit, so use a short home.
    let home = PathBuf::from(format!("/tmp/amx-sw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // Safe: this file has a single test.
    unsafe { std::env::set_var("AGENTMUX_HOME", &home) };
    let vault = Arc::new(Vault::open_at(&home.join("vault.db"), &home.join("threads")).unwrap());
    let core = Core::with_vault(vault);
    let sw = daemon::Switch::new(core.clone());

    sw.start().await.unwrap();
    assert!(sw.is_on() && config::socket_path().exists() && !config::is_paused());
    sw.start().await.unwrap(); // idempotent

    // A live connection works...
    let s = UnixStream::connect(config::socket_path()).await.unwrap();
    let (r, mut w) = s.into_split();
    w.write_all(b"{\"agentmux_hello\":{\"agent\":\"cursor\",\"cwd\":\"/tmp\",\"hop\":0}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n").await.unwrap();
    let mut lines = BufReader::new(r).lines();
    let first = tokio::time::timeout(Duration::from_secs(3), lines.next_line()).await.unwrap().unwrap().unwrap();
    assert!(first.contains("\"id\":1"), "{first}");

    // ...and turning off closes it, removes the socket and leaves the marker that stops respawns.
    sw.stop().await;
    assert!(!sw.is_on());
    let eof = tokio::time::timeout(Duration::from_secs(3), lines.next_line()).await.expect("connection should close").unwrap();
    assert!(eof.is_none(), "open connections are closed on off");
    assert!(UnixStream::connect(config::socket_path()).await.is_err());
    assert!(config::is_paused());
    let err = daemon::serve(core.clone()).await.unwrap_err().to_string();
    assert!(err.contains("turned off"), "the headless daemon must not start while off: {err}");

    // Turning it back on clears the marker and accepts connections again.
    sw.start().await.unwrap();
    assert!(!config::is_paused() && sw.is_on());
    assert!(UnixStream::connect(config::socket_path()).await.is_ok());

    // If something else already owns the socket, say so instead of pretending.
    sw.stop().await;
    let other = daemon::Switch::new(core.clone());
    other.start().await.unwrap();
    let third = daemon::Switch::new(core);
    let e = third.start().await.unwrap_err().to_string();
    assert!(e.contains("already running"), "{e}");
    other.stop().await;
    let _ = std::fs::remove_dir_all(&home);
}
