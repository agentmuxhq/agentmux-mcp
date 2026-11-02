use agentmux_core::{config, connectors, context, daemon, Core};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(name = "agentmux", version, about = "Local-first A2A & MCP context multiplexer")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the headless daemon (the desktop app embeds the same thing).
    Daemon,
    /// stdio MCP shim: this is what Cursor / Claude / Codex / Grok launch.
    Mcp {
        #[arg(long)]
        agent: String,
    },
    /// Register AgentMux as an MCP server in an agent (cursor|claude|chatgpt|grok|all).
    Connect { agent: String },
    /// Remove AgentMux from an agent's MCP config.
    Disconnect { agent: String },
    /// Show daemon state and provider connections.
    Status,
    /// Call an MCP tool through the daemon (for testing): agentmux call mux_contacts
    Call {
        tool: String,
        #[arg(default_value = "{}")]
        args: String,
        #[arg(long = "as", default_value = "user")]
        as_agent: String,
    },
    /// Run a command and publish its output to the room as shared terminal context.
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // The mcp shim speaks JSON-RPC on stdout, so all logging goes to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "agentmux_core=info,agentmux=info".into()))
        .init();
    match cli.cmd {
        Cmd::Daemon => daemon::serve(Core::new()?).await,
        Cmd::Mcp { agent } => shim(&agent).await,
        Cmd::Connect { agent } => each(&agent, connectors::install),
        Cmd::Disconnect { agent } => each(&agent, connectors::uninstall),
        Cmd::Status => status().await,
        Cmd::Call { tool, args, as_agent } => call(&as_agent, &tool, &args).await,
        Cmd::Run { cmd } => run(cmd).await,
    }
}

fn each(agent: &str, f: fn(&str) -> Result<String>) -> Result<()> {
    let ids: Vec<&str> = if agent == "all" { vec!["cursor", "claude", "chatgpt", "grok"] } else { vec![agent] };
    for id in ids {
        match f(id) {
            Ok(msg) => println!("✓ {id}: {msg}"),
            Err(e) => println!("✗ {id}: {e}"),
        }
    }
    Ok(())
}

async fn connect_or_spawn() -> Result<UnixStream> {
    let sock = config::socket_path();
    if let Ok(s) = UnixStream::connect(&sock).await {
        return Ok(s);
    }
    if config::is_paused() {
        bail!("AgentMux is turned off. Turn it on from the AgentMux menu bar icon, then try again.");
    }
    // No daemon yet: start a detached headless one, then wait for its socket.
    let mut c = std::process::Command::new(std::env::current_exe()?);
    c.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c.spawn().context("starting agentmux daemon")?;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(s) = UnixStream::connect(&sock).await {
            return Ok(s);
        }
    }
    bail!("agentmux daemon did not start (socket {})", sock.display())
}

fn hello(agent: &str) -> String {
    let hop: i64 = std::env::var("AGENTMUX_HOP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| ".".into());
    format!("{}\n", json!({"agentmux_hello": {"agent": agent, "cwd": cwd, "hop": hop}}))
}

async fn shim(agent: &str) -> Result<()> {
    let stream = connect_or_spawn().await?;
    let (mut r, mut w) = stream.into_split();
    w.write_all(hello(agent).as_bytes()).await?;
    let up = async move {
        let mut stdin = tokio::io::stdin();
        let _ = tokio::io::copy(&mut stdin, &mut w).await;
        let _ = w.shutdown().await; // let in-flight replies drain, then the daemon closes
    };
    let down = async move {
        let mut stdout = tokio::io::stdout();
        let _ = tokio::io::copy(&mut r, &mut stdout).await;
        let _ = stdout.flush().await;
    };
    tokio::join!(up, down);
    Ok(())
}

async fn rpc(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    w: &mut tokio::net::unix::OwnedWriteHalf,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value> {
    w.write_all(format!("{}\n", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).as_bytes()).await?;
    while let Some(line) = lines.next_line().await? {
        let v: Value = serde_json::from_str(&line)?;
        if v["id"] == id {
            return Ok(v);
        }
    }
    bail!("daemon closed the connection")
}

async fn call(agent: &str, tool: &str, args: &str) -> Result<()> {
    let stream = connect_or_spawn().await?;
    let (r, mut w) = stream.into_split();
    w.write_all(hello(agent).as_bytes()).await?;
    let mut lines = BufReader::new(r).lines();
    rpc(&mut lines, &mut w, 1, "initialize", json!({"protocolVersion": "2025-06-18"})).await?;
    let args: Value = serde_json::from_str(args).context("args must be JSON")?;
    let resp = rpc(&mut lines, &mut w, 2, "tools/call", json!({"name": tool, "arguments": args})).await?;
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    println!("{text}");
    if resp["result"]["isError"] == true {
        std::process::exit(1);
    }
    Ok(())
}

async fn status() -> Result<()> {
    let sock = config::socket_path();
    let up = UnixStream::connect(&sock).await.is_ok();
    println!("daemon: {} ({})", if up { "running" } else { "not running" }, sock.display());
    let core = Core::new()?;
    for p in core.vault.providers()? {
        let id = p["id"].as_str().unwrap_or("");
        let d = connectors::detect(id);
        println!(
            "  {:<8} app/cli: {:<5} registered: {:<5} handshake: {:<5} wake: {}",
            id,
            d["installed"],
            d["configured"],
            p["connectedAt"].is_string(),
            p["wakeKind"].as_str().unwrap_or("-")
        );
    }
    Ok(())
}

async fn run(cmd: Vec<String>) -> Result<()> {
    let out = tokio::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::inherit())
        .output()
        .await
        .with_context(|| format!("running {}", cmd[0]))?;
    let mut combined = String::from_utf8_lossy(&out.stdout).to_string();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    print!("{combined}");
    let code = out.status.code().unwrap_or(-1);
    let tail: String = combined.chars().rev().take(8000).collect::<Vec<_>>().into_iter().rev().collect();
    let body = format!("$ {}\n{}\n[exit {}]", cmd.join(" "), tail, code);

    let core = Core::new()?;
    let cwd = std::env::current_dir()?;
    let root = context::workspace_root(Path::new(&cwd));
    let (branch, _) = context::capture_git(&root);
    let (room, _) = core.vault.ensure_room(&root.to_string_lossy(), &branch)?;
    core.vault.set_context(&room, "terminal", &body)?;
    let _ = core.vault.export_markdown(&room);
    eprintln!("[agentmux] published terminal output to room {room}");
    std::process::exit(code);
}
