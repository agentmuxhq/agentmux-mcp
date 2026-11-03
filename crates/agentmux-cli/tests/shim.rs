//! The stdio shim survives a daemon restart: the agent's MCP connection keeps working, and a
//! request in flight when the daemon dies gets an error instead of hanging.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_agentmux");

fn daemon(home: &std::path::Path) -> Child {
    let c = Command::new(BIN).arg("daemon").env("AGENTMUX_HOME", home).env("AGENTMUX_PORT", "0").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let sock = home.join("run/agentmux.sock");
    let t = Instant::now();
    while std::os::unix::net::UnixStream::connect(&sock).is_err() {
        assert!(t.elapsed() < Duration::from_secs(10), "daemon did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    c
}

fn read(r: &mut impl BufRead) -> Value {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|_| panic!("bad line: {line:?}"))
}

#[test]
fn shim_reconnects_after_daemon_restart() {
    let home = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("shim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut d = daemon(&home);

    let mut shim = Command::new(BIN)
        .args(["mcp", "--agent", "claude"])
        .env("AGENTMUX_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut w = shim.stdin.take().unwrap();
    let mut r = BufReader::new(shim.stdout.take().unwrap());
    let mut send = |v: Value| writeln!(w, "{v}").unwrap();

    send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}));
    assert_eq!(read(&mut r)["result"]["serverInfo"]["name"], "agentmux");
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "mux_whoami", "arguments": {}}}));
    assert!(read(&mut r)["result"]["content"][0]["text"].as_str().unwrap().contains("claude"));

    // The daemon restarts (app update, takeover…). The shim must not exit.
    d.kill().unwrap();
    d.wait().unwrap();
    let _ = std::fs::remove_file(home.join("run/agentmux.sock"));
    let mut d2 = daemon(&home);

    send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "mux_whoami", "arguments": {}}}));
    let v = read(&mut r);
    assert_eq!(v["id"], 3);
    assert!(v["result"]["content"][0]["text"].as_str().unwrap_or("").contains("claude"), "{v}");
    assert!(shim.try_wait().unwrap().is_none(), "shim exited");

    d2.kill().unwrap();
    let _ = shim.kill();
}
