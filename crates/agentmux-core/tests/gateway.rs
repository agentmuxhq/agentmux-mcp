//! HTTP gateway + tunnel manager, with fake `cloudflared` / `ngrok` binaries (no network, no keychain).

use agentmux_core::gateway::{self, Gateway, REMOTE_TOOLS};
use agentmux_core::{CallerCtx, Core, Vault};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "agmx_test-token";
const TOKEN2: &str = "agmx_second-bot";
const PORT: u16 = 47317;

fn fake_bin(dir: &PathBuf, name: &str, script: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn rpc(c: &reqwest::Client, host: Option<&str>, token: Option<&str>, body: Value) -> reqwest::Response {
    let mut r = c.post(format!("http://127.0.0.1:{PORT}/mcp")).json(&body);
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    if let Some(h) = host {
        r = r.header("host", h);
    }
    r.send().await.unwrap()
}

async fn wait_state(gw: &Arc<Gateway>, want: &str) -> Value {
    for _ in 0..100 {
        let s = gw.status();
        if s["state"] == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("tunnel never reached {want}: {}", gw.status());
}

#[test]
fn url_parsing() {
    let cf = "2026-01-01T00:00:00Z INF |  https://quiet-river-1234.trycloudflare.com                |";
    assert_eq!(gateway::find_public_url("cloudflared", cf).as_deref(), Some("https://quiet-river-1234.trycloudflare.com"));
    assert_eq!(gateway::find_public_url("cloudflared", "INF Requesting new quick Tunnel on https://api.trycloudflare.com..."), None);
    assert_eq!(gateway::find_public_url("cloudflared", "INF connected to https://example.com"), None);
    let ng = r#"{"lvl":"info","msg":"started tunnel","url":"https://abc.ngrok-free.app"}"#;
    assert_eq!(gateway::find_public_url("ngrok", ng).as_deref(), Some("https://abc.ngrok-free.app"));
    assert_eq!(gateway::find_public_url("ngrok", r#"{"msg":"starting web service","url":"127.0.0.1:4040"}"#), None);
}

#[test]
fn routed_hostname_parsing() {
    let added = "2026-11-02T06:00:00Z INF Added CNAME mux.example.com which will route to this tunnel tunnelID=abc";
    assert_eq!(gateway::routed_hostname(added).as_deref(), Some("mux.example.com"));
    let existing = "2026-11-02T06:00:00Z INF mux.example.com is already configured to route to your tunnel tunnelID=abc";
    assert_eq!(gateway::routed_hostname(existing).as_deref(), Some("mux.example.com"));
    assert_eq!(gateway::routed_hostname("INF something else"), None);
}

#[tokio::test]
async fn gateway_end_to_end() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bins = dir.join("bin");
    std::fs::create_dir_all(&bins).unwrap();
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    // Prints its URL like cloudflared and serves /quicktunnel on the --metrics port,
    // so a relaunched gateway can adopt it. The server exits when the script does.
    fake_bin(
        &bins,
        "cloudflared",
        r#"echo '2026 INF |  https://fake-name-1.trycloudflare.com  |' >&2
while [ $# -gt 0 ]; do [ "$1" = --metrics ] && M=$2; shift; done
python3 -c '
import http.server, json, os, sys, threading, time
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(s):
        b = json.dumps({"hostname": "fake-name-1.trycloudflare.com"}).encode()
        s.send_response(200); s.send_header("content-type", "application/json"); s.end_headers(); s.wfile.write(b)
    def log_message(*a): pass
srv = http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
p = os.getppid()
while os.getppid() == p: time.sleep(0.2)
' "${M##*:}" &
wait"#,
    );
    fake_bin(
        &bins,
        "ngrok",
        "echo '{\"lvl\":\"info\",\"msg\":\"started tunnel\",\"url\":\"https://fake.ngrok-free.app\"}'\nexec sleep 300",
    );
    // Safe: set once, before this test reads them.
    unsafe {
        std::env::set_var("AGENTMUX_SECRET_GROK_MCP_TOKEN", TOKEN);
        std::env::set_var("AGENTMUX_SECRET_GROK_MCP_TOKEN_JOB_HUNTER", TOKEN2);
        std::env::set_var("TUNNEL_ORIGIN_CERT", dir.join("cert.pem"));
        std::env::set_var("AGENTMUX_PORT", PORT.to_string());
        std::env::set_var("PATH", format!("{}:{}", bins.display(), std::env::var("PATH").unwrap()));
    }
    let vault = Arc::new(Vault::open_at(&dir.join("vault.db"), &dir.join("threads")).unwrap());
    let core = Core::with_vault(vault);
    let gw = Gateway::new(core.clone());
    let server = gw.clone();
    tokio::spawn(async move { server.serve_http().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let c = reqwest::Client::new();

    // ── auth ──
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
    assert_eq!(rpc(&c, None, None, init.clone()).await.status(), 401, "no token");
    assert_eq!(rpc(&c, None, Some("wrong"), init.clone()).await.status(), 401, "bad token");
    assert_eq!(rpc(&c, Some("evil.example.com"), Some(TOKEN), init.clone()).await.status(), 403, "foreign Host");
    let ok = rpc(&c, None, Some(TOKEN), init).await;
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.json::<Value>().await.unwrap()["result"]["serverInfo"]["name"], "agentmux");
    assert_eq!(rpc(&c, None, Some(TOKEN), json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await.status(), 202);

    // ── scope: only the answering tools ──
    let list = rpc(&c, None, Some(TOKEN), json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await.json::<Value>().await.unwrap();
    let mut names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    let mut want = REMOTE_TOOLS.to_vec();
    want.sort();
    assert_eq!(names, want);
    let call = |name: &str, args: Value| json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":args}});
    let denied = rpc(&c, None, Some(TOKEN), call("mux_ask_peer", json!({"to":"claude","question":"x"}))).await.json::<Value>().await.unwrap();
    assert_eq!(denied["result"]["isError"], true);
    assert!(denied["result"]["content"][0]["text"].as_str().unwrap().contains("not available"));
    let denied = rpc(&c, None, Some(TOKEN), call("mux_read_context", json!({}))).await.json::<Value>().await.unwrap();
    assert_eq!(denied["result"]["isError"], true);

    // ── a real question answered over HTTP ──
    let cursor = CallerCtx { agent: "cursor".into(), cwd: dir.join("ws").to_string_lossy().to_string(), hop: 0 };
    let ticket = core.start_ticket(&cursor, None, "grok", "whats up with the job hunter bot?", None).unwrap();
    let inbox = rpc(&c, None, Some(TOKEN), call("mux_inbox", json!({}))).await.json::<Value>().await.unwrap();
    let text = inbox["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(&ticket) && text.contains("job hunter bot"), "{text}");
    let reply = rpc(&c, None, Some(TOKEN), call("mux_reply", json!({"ticket": ticket, "text": "all green"}))).await.json::<Value>().await.unwrap();
    let rt = reply["result"]["content"][0]["text"].as_str().unwrap().replace(' ', "");
    assert!(rt.contains("\"accepted\":true"), "{rt}");
    let t = core.wait_ticket(&ticket, 1).await.unwrap();
    assert_eq!(t["status"], "answered");
    assert_eq!(t["answer"], "all green");

    // ── a second bot has its own identity, inbox and name ──
    let bot = core.vault.add_bot("Job Hunter").unwrap();
    assert_eq!(bot, "grok:job-hunter");
    let t_bot = core.start_ticket(&cursor, None, &bot, "private to the job hunter", None).unwrap();
    let inbox2 = rpc(&c, None, Some(TOKEN2), call("mux_inbox", json!({}))).await.json::<Value>().await.unwrap();
    let text2 = inbox2["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text2.contains(&t_bot) && !text2.contains("job hunter bot?"), "{text2}");
    let inbox1 = rpc(&c, None, Some(TOKEN), call("mux_inbox", json!({}))).await.json::<Value>().await.unwrap();
    assert!(!inbox1["result"]["content"][0]["text"].as_str().unwrap().contains(&t_bot), "first bot must not see the second bot's ticket");
    let stolen = rpc(&c, None, Some(TOKEN), call("mux_reply", json!({"ticket": t_bot, "text": "hijack"}))).await.json::<Value>().await.unwrap();
    assert_eq!(stolen["result"]["isError"], true, "bot A cannot answer bot B's ticket");
    let who = rpc(&c, None, Some(TOKEN2), call("mux_whoami", json!({}))).await.json::<Value>().await.unwrap();
    assert!(who["result"]["content"][0]["text"].as_str().unwrap().replace(' ', "").contains("\"agent\":\"grok:job-hunter\""));
    let reg = rpc(&c, None, Some(TOKEN2), call("mux_register", json!({"name": "Job Hunter Pro", "description": "finds remote jobs"}))).await.json::<Value>().await.unwrap();
    assert_eq!(reg["result"]["isError"], Value::Null);
    let me = core.vault.providers().unwrap().into_iter().find(|p| p["id"] == bot.as_str()).unwrap();
    assert_eq!(me["name"], "Job Hunter Pro");
    assert_eq!(me["registered"], true);
    assert_eq!(rpc(&c, None, Some(TOKEN2), call("mux_ask_peer", json!({"to":"claude","question":"x"}))).await.json::<Value>().await.unwrap()["result"]["isError"], true);
    let ok2 = rpc(&c, None, Some(TOKEN2), call("mux_reply", json!({"ticket": t_bot, "text": "mine"}))).await.json::<Value>().await.unwrap();
    assert_ne!(ok2["result"]["isError"], true);
    assert_eq!(core.wait_ticket(&t_bot, 1).await.unwrap()["answer"], "mine");

    // ── tunnel: cloudflared is the default ──
    let s = gw.status();
    assert_eq!(s["provider"], "cloudflared");
    assert_eq!(s["state"], "stopped");
    gw.start().await.unwrap();
    let s = wait_state(&gw, "running").await;
    assert_eq!(s["mcpUrl"], "https://fake-name-1.trycloudflare.com/mcp");
    // The tunnel's own hostname is accepted, others are not.
    assert_eq!(rpc(&c, Some("fake-name-1.trycloudflare.com"), Some(TOKEN), json!({"jsonrpc":"2.0","id":9,"method":"ping"})).await.status(), 200);
    assert_eq!(rpc(&c, Some("other.trycloudflare.com"), Some(TOKEN), json!({"jsonrpc":"2.0","id":9,"method":"ping"})).await.status(), 403);
    assert!(gw.start().await.is_err(), "double start");
    assert!(gw.set_provider("ngrok", None).is_err(), "cannot switch while running");
    // ── a relaunched app adopts the still-running tunnel, URL and all ──
    tokio::time::sleep(Duration::from_millis(500)).await; // let the fake metrics server bind
    let relaunched = Gateway::new(core.clone());
    assert!(relaunched.adopt().await, "should find the running tunnel");
    let s2 = relaunched.status();
    assert_eq!((s2["state"].as_str(), s2["mcpUrl"].as_str()), (Some("running"), Some("https://fake-name-1.trycloudflare.com/mcp")));
    relaunched.stop().await;
    assert_eq!(relaunched.status()["state"], "stopped");
    wait_state(&gw, "stopped").await; // the original supervisor sees its child go too
    assert!(!Gateway::new(core.clone()).adopt().await, "nothing left to adopt");
    gw.stop().await;
    assert_eq!(gw.status()["state"], "stopped");
    assert_eq!(gw.status()["url"], Value::Null);

    // ── switch to ngrok ──
    assert!(gw.set_provider("bogus", None).is_err());
    assert!(gw.set_provider("ngrok", Some("bad domain/..")).is_err());
    gw.set_provider("ngrok", Some("my-app.ngrok-free.app")).unwrap();
    gw.start().await.unwrap();
    let s = wait_state(&gw, "running").await;
    assert_eq!(s["mcpUrl"], "https://fake.ngrok-free.app/mcp");
    gw.stop().await;

    // ── a tunnel that dies before publishing reports why ──
    fake_bin(
        &bins,
        "ngrok",
        "echo '{\"lvl\":\"eror\",\"err\":\"authentication failed: ERR_NGROK_4018\"}'\nexit 1",
    );
    gw.start().await.unwrap();
    let s = wait_state(&gw, "error").await;
    assert!(s["error"].as_str().unwrap().contains("ERR_NGROK_4018"), "{s}");

    // ── permanent Cloudflare tunnel (named, in the user's account) ──
    gw.stop().await;
    gw.set_provider("cloudflared", None).unwrap();
    assert_eq!(gw.status()["permanent"], false);
    assert!(gw.set_autostart(true).is_err(), "a temporary URL can't auto-start");
    assert!(gw.cf_create("mux.example.com").await.unwrap_err().to_string().contains("connect Cloudflare"));
    std::fs::write(dir.join("cert.pem"), "fake").unwrap();
    assert_eq!(gw.status()["cfLoggedIn"], true);
    gw.cf_login().await.unwrap(); // already logged in: no browser
    assert!(gw.cf_create("https://mux.example.com/x").await.is_err(), "hostname only");
    let d = dir.display();
    fake_bin(
        &bins,
        "cloudflared",
        &format!(
            r#"case "$*" in
  *"tunnel list"*) if [ -f {d}/created ]; then echo '[{{"id":"1111-2222","name":"agentmux-mux-example-com","deleted_at":"0001-01-01T00:00:00Z"}}]'; else echo '[]'; fi ;;
  *"tunnel create"*) touch {d}/created; echo created ;;
  *"route dns"*) echo "$*" > {d}/route; echo "2026 INF Added CNAME mux.example.com which will route to this tunnel tunnelID=1111-2222" >&2 ;;
  *" run "*) echo "$*" > {d}/runargs; echo "2026 INF Registered tunnel connection connIndex=0" >&2; trap 'kill $!' TERM; sleep 300 & wait ;;
esac"#
        ),
    );
    // Just the subdomain: Cloudflare picks the domain chosen at login, and we read it back.
    assert_eq!(gw.cf_create("Mux").await.unwrap(), "mux.example.com");
    let route = std::fs::read_to_string(dir.join("route")).unwrap();
    assert!(route.contains("route dns --overwrite-dns 1111-2222 mux"), "{route}");
    assert_eq!(gw.status()["cfZone"], "example.com");
    assert!(gw.cf_create("bad label!").await.is_err());
    assert_eq!(gw.cf_create("mux.example.com").await.unwrap(), "mux.example.com"); // full hostname works too
    // A domain that isn't on the connected Cloudflare zone is refused, not silently mis-reported.
    let e = gw.cf_create("mux.other.org").await.unwrap_err().to_string();
    assert!(e.contains("isn't on the Cloudflare domain") && e.contains("mux.example.com"), "{e}");
    let s = gw.status();
    assert_eq!((s["permanent"].as_bool(), s["cfMode"].as_str(), s["cfHostname"].as_str()), (Some(true), Some("named"), Some("mux.example.com")));
    gw.set_autostart(true).unwrap();
    gw.start().await.unwrap();
    let s = wait_state(&gw, "running").await;
    assert_eq!(s["mcpUrl"], "https://mux.example.com/mcp", "URL is known up front");
    let runargs = std::fs::read_to_string(dir.join("runargs")).unwrap();
    assert!(runargs.contains(&format!("run --url http://127.0.0.1:{PORT} 1111-2222")), "{runargs}");
    assert_eq!(rpc(&c, Some("mux.example.com"), Some(TOKEN), json!({"jsonrpc":"2.0","id":9,"method":"ping"})).await.status(), 200);
    assert!(gw.cf_reset().is_err(), "can't switch mode while running");
    // A relaunched app adopts the permanent tunnel by its hostname.
    let relaunched = Gateway::new(core.clone());
    assert!(relaunched.adopt().await);
    assert_eq!(relaunched.status()["mcpUrl"], "https://mux.example.com/mcp");
    relaunched.stop().await;
    wait_state(&gw, "stopped").await;
    gw.cf_reset().unwrap();
    let s = gw.status();
    assert_eq!((s["permanent"].as_bool(), s["autostart"].as_bool(), s["cfMode"].as_str()), (Some(false), Some(false), Some("quick")));

    // ── brute force is throttled ──
    for _ in 0..12 {
        rpc(&c, None, Some("guess"), json!({"jsonrpc":"2.0","id":1,"method":"ping"})).await;
    }
    assert_eq!(rpc(&c, None, Some(TOKEN), json!({"jsonrpc":"2.0","id":1,"method":"ping"})).await.status(), 429);
}
