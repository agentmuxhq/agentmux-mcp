//! The HTTP port follows the socket: a process serves it only while it owns the socket, and gives it
//! back the moment it doesn't, so the app and the background service can't split the two.

use agentmux_core::gateway::Gateway;
use agentmux_core::{Core, Vault};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn until(what: &str, secs: u64, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn http_port_follows_socket_ownership() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("httpown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    // SAFETY: the only test in this binary.
    unsafe { std::env::set_var("AGENTMUX_PORT", port.to_string()) };

    let vault = Arc::new(Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap());
    let gw = Gateway::new(Core::with_vault(vault));
    let owns = Arc::new(AtomicBool::new(false));
    let o = owns.clone();
    tokio::spawn(gw.clone().serve_http_while(move || o.load(Ordering::SeqCst)));

    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!gw.http_bound(), "must not serve the port without owning the socket");
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(), "port should be free");

    owns.store(true, Ordering::SeqCst);
    until("serving after becoming the owner", 6, || gw.http_bound()).await;
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err(), "port should be taken");

    owns.store(false, Ordering::SeqCst);
    until("releasing the port after losing ownership", 6, || !gw.http_bound()).await;
    until("the port actually being free", 3, || std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()).await;

    // Another process holds the port: keep retrying, and serve once it's free.
    let blocker = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    owns.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!gw.http_bound(), "port is busy");
    drop(blocker);
    until("taking the port once it is free", 8, || gw.http_bound()).await;

    // An explicit release (the app's button) frees it too; ownership then re-serves it.
    gw.release_http();
    assert!(!gw.http_bound());
    until("port free after release", 3, || std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()).await;
}
