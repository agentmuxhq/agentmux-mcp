use agentmux_core::gateway::Gateway;
use agentmux_core::{config, connectors, context, sync, webhook, Core};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

mod hook;
mod shim;
mod service_mac;

#[derive(Parser)]
#[command(
    name = "agentmux",
    version,
    about = "Let your AI agents message each other. One local MCP server for Cursor, Claude Code, Codex and Grok bots.",
    after_help = "Get started: agentmux init"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Set up this Mac: connect your agents and run AgentMux in the background at login.
    Init {
        /// Don't install the login service (run `agentmux daemon` yourself).
        #[arg(long)]
        no_service: bool,
        /// Don't write MCP configs for Cursor, Claude Code and Codex.
        #[arg(long)]
        no_connect: bool,
    },
    /// Turn AgentMux on (starts the background service).
    Start,
    /// Turn AgentMux off: stops the daemon and the tunnel. Nothing restarts it until `agentmux start`.
    Stop,
    /// Show what's running: daemon, tunnel, agents, sync.
    Status,
    /// Public access for cloud agents (Grok bots): status | start | stop | test | restore | keep-temporary.
    Tunnel {
        #[arg(default_value = "status")]
        action: String,
    },
    /// Grok bots: list | add <name> | remove <id> | token <id> [--rotate] | webhook <id> <url> | ask <id> on|off
    Bot {
        action: String,
        args: Vec<String>,
        #[arg(long)]
        rotate: bool,
    },
    /// Back up to your own private git repo: status | connect <remote> | now | off
    Sync {
        #[arg(default_value = "status")]
        action: String,
        remote: Option<String>,
        /// Include git diffs and terminal output (off by default).
        #[arg(long)]
        diffs: bool,
        /// Don't sync automatically every 15 minutes.
        #[arg(long)]
        manual: bool,
    },
    /// Remove the login service (and, unless --keep-configs, AgentMux from your agents' MCP configs).
    Uninstall {
        #[arg(long)]
        keep_configs: bool,
    },
    /// Run the daemon in the foreground (this is what the login service runs).
    Daemon,
    /// stdio MCP shim: this is what Cursor / Claude / Codex / Grok launch.
    Mcp {
        #[arg(long)]
        agent: String,
    },
    /// Register AgentMux as an MCP server in an agent (cursor|claude|chatgpt|grok|<custom>|all).
    Connect {
        agent: String,
        /// Claude Code: don't add the context hook to ~/.claude/settings.json.
        #[arg(long)]
        no_hook: bool,
    },
    /// Agent hooks. `agentmux hook claude-code` is what Claude Code runs (prints what's new);
    /// `--on` / `--off` add or remove the hook.
    Hook {
        target: String,
        #[arg(long)]
        on: bool,
        #[arg(long)]
        off: bool,
    },
    /// Custom providers from TOML manifests: list | add <file.toml> | remove <id>
    Provider {
        #[arg(default_value = "list")]
        action: String,
        arg: Option<String>,
    },
    /// Remove AgentMux from an agent's MCP config.
    Disconnect { agent: String },
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
    let Some(cmd) = cli.cmd else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    match cmd {
        Cmd::Init { no_service, no_connect } => init(no_service, no_connect).await,
        Cmd::Start => start().await,
        Cmd::Stop => stop().await,
        Cmd::Status => status().await,
        Cmd::Tunnel { action } => tunnel(&action).await,
        Cmd::Bot { action, args, rotate } => bot(&action, &args, rotate).await,
        Cmd::Sync { action, remote, diffs, manual } => sync_cmd(&action, remote.as_deref(), diffs, manual).await,
        Cmd::Uninstall { keep_configs } => uninstall(keep_configs),
        Cmd::Daemon => agentmux_core::service::run_headless(Core::new()?).await,
        Cmd::Mcp { agent } => shim::run(&agent).await,
        Cmd::Connect { agent, no_hook } => {
            each(&agent, connectors::install)?;
            if no_hook && (agent == "claude" || agent == "all") {
                println!("{}", connectors::set_option("claude", "hook", false)?);
            }
            Ok(())
        }
        Cmd::Hook { target, on, off } => hook::run(&target, on, off),
        Cmd::Provider { action, arg } => provider_cmd(&action, arg.as_deref()),
        Cmd::Disconnect { agent } => each(&agent, |id| {
            let msg = connectors::uninstall(id)?;
            if let Ok(v) = agentmux_core::Vault::open() {
                let _ = v.clear_connection(id);
            }
            Ok(msg)
        }),
        Cmd::Call { tool, args, as_agent } => call(&as_agent, &tool, &args).await,
        Cmd::Run { cmd } => run(cmd).await,
    }
}

fn each(agent: &str, f: fn(&str) -> Result<String>) -> Result<()> {
    let ids: Vec<String> = if agent == "all" { config::agent_ids() } else { vec![agent.to_string()] };
    for id in ids.iter().map(String::as_str) {
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

// ───────── setup and on/off ─────────

async fn wait_for_daemon(secs: u64) -> bool {
    for _ in 0..(secs * 4) {
        if UnixStream::connect(config::socket_path()).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

async fn init(no_service: bool, no_connect: bool) -> Result<()> {
    println!("Setting up AgentMux on this Mac\n");
    let bin = connectors::ensure_bin_link()?;
    println!("✓ agentmux available at {}", bin.display());

    if !no_connect {
        let ids: Vec<String> = agentmux_core::providers::all().iter().filter(|p| p.caps().wake == "cli").map(|p| p.info().id).collect();
        for id in ids.iter().map(String::as_str) {
            let d = connectors::detect(id);
            let name = config::agent_name(id);
            if d["installed"] != true {
                println!("· {name}: not found on this Mac, skipped");
                continue;
            }
            match connectors::install(id) {
                Ok(_) => println!("✓ {name}: connected"),
                Err(e) => println!("✗ {name}: {e}"),
            }
        }
    }

    let _ = std::fs::remove_file(config::paused_marker());
    if no_service {
        println!("· login service skipped; run `agentmux daemon` to start AgentMux");
    } else if UnixStream::connect(config::socket_path()).await.is_ok() && !service_mac::loaded() {
        // The desktop app is already serving; the service still installs and stands by behind it.
        service_mac::install(&bin)?;
        println!("✓ background service installed (the AgentMux app is running and serving for now)");
    } else {
        service_mac::install(&bin)?;
        if wait_for_daemon(10).await {
            println!("✓ AgentMux is running and will start at login");
        } else {
            println!("✗ the service was installed but didn't start; see {}", config::home().join("logs/daemon.log").display());
        }
    }

    println!(
        "\nNext:\n  1. Restart Cursor, Claude Code and Codex so they load AgentMux.\n  2. In any of them, try: \"Use the agentmux tool mux_contacts and tell me who is connected.\"\n  3. Grok bots: agentmux bot add \"My Bot\", then agentmux tunnel start (see agentmux bot --help).\n\nagentmux status shows what's running. agentmux stop turns everything off."
    );
    Ok(())
}

async fn start() -> Result<()> {
    let _ = std::fs::remove_file(config::paused_marker());
    if service_mac::installed() {
        service_mac::start()?;
    } else {
        spawn_daemon()?;
    }
    if wait_for_daemon(10).await {
        println!("✓ AgentMux is on");
    } else {
        bail!("AgentMux didn't start; see {}", config::home().join("logs/daemon.log").display());
    }
    Ok(())
}

async fn stop() -> Result<()> {
    // Take the tunnel down first so nothing stays exposed, then mark AgentMux off and stop the service.
    let _ = control("tunnel.stop").await;
    std::fs::create_dir_all(config::run_dir())?;
    std::fs::write(config::paused_marker(), "off\n")?;
    service_mac::stop()?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    // The daemon (headless or the app) notices the marker within a few seconds.
    for _ in 0..24 {
        if UnixStream::connect(config::socket_path()).await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if UnixStream::connect(config::socket_path()).await.is_ok() {
        println!("AgentMux is marked off, but something is still serving (an older AgentMux app?). Quit it from its menu bar icon.");
    } else {
        println!("✓ AgentMux is off. Agents can't reach it until you run `agentmux start`.");
    }
    Ok(())
}

fn uninstall(keep_configs: bool) -> Result<()> {
    service_mac::uninstall()?;
    println!("✓ login service removed");
    if !keep_configs {
        each("all", connectors::uninstall)?;
    }
    println!("Your data stays in {}. Delete that folder to remove it.", config::home().display());
    Ok(())
}

fn spawn_daemon() -> Result<()> {
    let mut c = std::process::Command::new(std::env::current_exe()?);
    c.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c.spawn().context("starting agentmux daemon")?;
    Ok(())
}

// ───────── status ─────────

async fn status() -> Result<()> {
    let up = UnixStream::connect(config::socket_path()).await.is_ok();
    let off = config::is_paused();
    let who = if service_mac::loaded() { "background service" } else { "the AgentMux app or a manual daemon" };
    println!(
        "AgentMux {}  {}",
        env!("CARGO_PKG_VERSION"),
        match (up, off) {
            (true, _) => format!("● on (served by {who})"),
            (false, true) => "○ off (agentmux start turns it on)".into(),
            (false, false) => "○ not running (agentmux start)".into(),
        }
    );
    if let Ok(t) = control("tunnel.status").await {
        let state = t["state"].as_str().unwrap_or("?");
        let mode = if t["permanent"] == true { "permanent" } else { "temporary" };
        println!("Tunnel   {} · {}{}", state, mode, t["mcpUrl"].as_str().map(|u| format!(" · {u}")).unwrap_or_default());
    }
    let core = Core::new()?;
    println!("Agents");
    for p in core.vault.providers()? {
        let id = p["id"].as_str().unwrap_or("");
        let seen = p["lastSeen"].as_str().unwrap_or("never");
        println!(
            "  {} {:<14} {:<10} wake: {:<8} last seen {}",
            if p["connectedAt"].is_string() { "●" } else { "○" },
            p["name"].as_str().unwrap_or(id),
            id,
            p["wakeKind"].as_str().unwrap_or("-"),
            seen
        );
    }
    let s = sync::status(&core.vault);
    if s["configured"] == true {
        println!("Sync     {} · last {}", s["remote"].as_str().unwrap_or(""), s["last"].as_str().unwrap_or("never"));
    }
    Ok(())
}

// ───────── tunnel (talks to the running daemon) ─────────

async fn control(cmd: &str) -> Result<Value> {
    agentmux_core::daemon::control_request(cmd).await
}

async fn tunnel(action: &str) -> Result<()> {
    let cmd = match action {
        "status" => "tunnel.status",
        "start" | "on" => "tunnel.start",
        "stop" | "off" => "tunnel.stop",
        "test" | "verify" => "tunnel.verify",
        "restore" => "tunnel.restore",
        "keep-temporary" => "tunnel.dismiss_lost",
        _ => bail!("use: agentmux tunnel status | start | stop | test | restore | keep-temporary"),
    };
    let r = control(cmd).await?;
    if cmd == "tunnel.restore" {
        println!("✓ Restored the permanent tunnel {}. Start it with: agentmux tunnel start", r.as_str().unwrap_or(""));
        return Ok(());
    }
    if cmd == "tunnel.verify" {
        println!("✓ {}", r.as_str().unwrap_or("reachable"));
        return Ok(());
    }
    if cmd == "tunnel.start" {
        // Wait for a temporary tunnel to report its address.
        for _ in 0..40 {
            if let Ok(s) = control("tunnel.status").await {
                if s["state"] != "starting" {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    let t = control("tunnel.status").await?;
    println!("Tunnel: {}", t["state"].as_str().unwrap_or("?"));
    println!("  provider: {} ({})", t["provider"].as_str().unwrap_or("?"), if t["permanent"] == true { "permanent" } else { "temporary URL" });
    if let Some(u) = t["mcpUrl"].as_str() {
        println!("  MCP URL:  {u}");
    }
    if let Some(e) = t["error"].as_str() {
        println!("  error:    {e}");
    }
    if let Some(h) = t["lostPermanent"].as_str() {
        println!("  ⚠ your permanent tunnel {h} was cleared from the settings.");
        println!("    Restore it: agentmux tunnel restore   (or use a temporary one: agentmux tunnel keep-temporary)");
    } else if t["permanent"] != true {
        println!("  Set up a permanent URL in the AgentMux app: Settings → Public access.");
    }
    Ok(())
}

// ───────── Grok bots ─────────

fn read_secret(prompt: &str) -> Result<String> {
    use std::io::{BufRead, IsTerminal};
    let tty = std::io::stdin().is_terminal();
    if tty {
        eprint!("{prompt}");
        let _ = std::process::Command::new("stty").arg("-echo").status();
    }
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if tty {
        let _ = std::process::Command::new("stty").arg("echo").status();
        eprintln!();
    }
    Ok(line.trim().to_string())
}

async fn bot(action: &str, args: &[String], rotate: bool) -> Result<()> {
    let core = Core::new()?;
    let arg = |i: usize, what: &str| args.get(i).cloned().ok_or_else(|| anyhow::anyhow!("missing {what}"));
    match action {
        "list" | "ls" => {
            for p in core.vault.providers()?.into_iter().filter(|p| p["family"] == "grok") {
                println!(
                    "{} {:<20} {:<22} wake: {:<8} start conversations: {}",
                    if p["connectedAt"].is_string() { "●" } else { "○" },
                    p["name"].as_str().unwrap_or(""),
                    p["id"].as_str().unwrap_or(""),
                    p["wakeKind"].as_str().unwrap_or("-"),
                    if p["canAsk"] == true { "on" } else { "off" }
                );
            }
        }
        "add" => {
            let id = core.vault.add_bot(&args.join(" "))?;
            println!("✓ added {id}\nNext: agentmux bot token {id}   (paste the URL and token into Grok's MCP connector)\n      agentmux bot webhook {id} <url>   (so AgentMux can wake it)");
        }
        "remove" | "rm" => {
            let id = arg(0, "bot id")?;
            core.vault.remove_bot(&id)?;
            let _ = webhook::clear(&core.vault, &id);
            Gateway::new(core.clone()).forget(&id);
            println!("✓ removed {id}");
        }
        "token" => {
            let id = arg(0, "bot id")?;
            let gw = Gateway::new(core.clone());
            let t = if rotate { gw.rotate_token(&id)? } else { gw.token(&id)? };
            let url = control("tunnel.status").await.ok().and_then(|s| s["mcpUrl"].as_str().map(String::from));
            println!("MCP URL: {}", url.unwrap_or_else(|| "(start the tunnel first: agentmux tunnel start)".into()));
            println!("Token:   {t}");
            if rotate {
                println!("The old token stops working immediately. Update it in Grok's connector.");
            }
        }
        "webhook" => {
            let id = arg(0, "bot id")?;
            let url = arg(1, "webhook URL")?;
            let key = read_secret("Webhook key (input hidden): ")?;
            webhook::save(&core.vault, &id, &url, &key)?;
            println!("✓ {id} is now woken by its webhook. Routine instructions to paste into Grok:\n\n{}", webhook::instructions(&id, core.vault.can_ask(&id)));
        }
        "ask" => {
            let id = arg(0, "bot id")?;
            let on = matches!(args.get(1).map(String::as_str), Some("on") | Some("yes") | Some("true"));
            core.vault.set_can_ask(&id, on)?;
            println!("✓ {id} {} start conversations", if on { "can" } else { "can no longer" });
        }
        _ => bail!("use: agentmux bot list | add <name> | remove <id> | token <id> [--rotate] | webhook <id> <url> | ask <id> on|off"),
    }
    Ok(())
}

// ───────── sync ─────────

async fn sync_cmd(action: &str, remote: Option<&str>, diffs: bool, manual: bool) -> Result<()> {
    let core = Core::new()?;
    let v = &core.vault;
    let report = |r: &Value| {
        let i = &r["imported"];
        println!(
            "✓ synced: brought in {} messages, {} sessions, {} rooms; {}",
            i["messages"],
            i["sessions"],
            i["rooms"],
            if r["pushed"] == true { "backed up your latest changes" } else { "already up to date" }
        );
    };
    match action {
        "status" => {
            let s = sync::status(v);
            if s["configured"] != true {
                println!("Sync is off. Set it up with: agentmux sync connect git@github.com:you/agentmux-backup.git");
            } else {
                println!("Remote:  {}", s["remote"].as_str().unwrap_or(""));
                println!("Last:    {}", s["last"].as_str().unwrap_or("never"));
                println!("Auto:    {}   Diffs: {}", if s["auto"] == true { "every 15 min" } else { "off" }, if s["includeDiffs"] == true { "included" } else { "not included" });
                if let Some(e) = s["error"].as_str() {
                    println!("Error:   {e}");
                }
            }
        }
        "connect" => {
            let r = remote.ok_or_else(|| anyhow::anyhow!("give the repo: agentmux sync connect git@github.com:you/agentmux-backup.git"))?;
            sync::connect(v, r, diffs, !manual).await?;
            report(&sync::sync_now(v).await?);
        }
        "now" => report(&sync::sync_now(v).await?),
        "off" | "disconnect" => {
            sync::disconnect(v)?;
            println!("✓ sync is off (your data stays here and in the repo)");
        }
        _ => bail!("use: agentmux sync status | connect <remote> [--diffs] [--manual] | now | off"),
    }
    Ok(())
}

fn provider_cmd(action: &str, arg: Option<&str>) -> Result<()> {
    use agentmux_core::providers::{self, manifest};
    match (action, arg) {
        ("list", _) => {
            for p in providers::all() {
                let i = p.info();
                let c = p.caps();
                let custom = !providers::builtin().iter().any(|b| b.info().id == i.id);
                println!("{:<10} {:<14} wake: {:<8}{}", i.id, i.name, c.wake, if custom { " (custom)" } else { "" });
            }
            println!("\nCustom providers live in {}", manifest::dir().display());
            Ok(())
        }
        ("add", Some(file)) => {
            let id = manifest::add(Path::new(file))?;
            println!("✓ added provider '{id}'. Restart AgentMux, then: agentmux connect {id}");
            Ok(())
        }
        ("remove", Some(id)) => {
            manifest::remove(id)?;
            println!("✓ removed provider '{id}'");
            Ok(())
        }
        _ => bail!("usage: agentmux provider list | add <file.toml> | remove <id>"),
    }
}
