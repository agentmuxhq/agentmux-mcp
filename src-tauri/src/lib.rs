use agentmux_core::gateway::Gateway;
use agentmux_core::{config, connectors, context, daemon, mcp, sync, vault, webhook, CallerCtx, Core};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, State, WindowEvent};
use tauri_plugin_updater::UpdaterExt;

mod tray;

pub(crate) struct AppState {
    pub(crate) core: Arc<Core>,
    pub(crate) gateway: Arc<Gateway>,
    pub(crate) daemon: Arc<Mutex<String>>,
    switch: Arc<daemon::Switch>,
    /// Newest version GitHub offered, if it is newer than this one.
    pub(crate) update: Arc<Mutex<Option<String>>>,
    /// What the tray menu currently shows, so it is only rebuilt on change.
    tray_sig: Mutex<Option<Vec<tray::E>>>,
}

/// Turn AgentMux on or off: the daemon, the HTTP gateway and the tunnel go together.
pub(crate) async fn power(app: &tauri::AppHandle, on: bool) -> Result<String, String> {
    let st = app.state::<AppState>();
    if on {
        let status = match st.switch.start().await {
            Ok(()) => "embedded".to_string(),
            Err(e) if e.to_string().contains("already running") => "external".to_string(),
            Err(e) => format!("error: {e}"),
        };
        *st.daemon.lock().unwrap() = status.clone();
        st.gateway.set_paused(false);
        if st.gateway.autostart() && st.gateway.permanent() && !matches!(st.gateway.status()["state"].as_str(), Some("running") | Some("starting")) {
            let _ = st.gateway.start().await;
        }
        st.core.emit("daemon", json!({"on": true}));
        tray::refresh(app);
        Ok(status)
    } else {
        st.switch.stop().await;
        st.gateway.set_paused(true);
        st.gateway.stop().await;
        *st.daemon.lock().unwrap() = "off".into();
        st.core.emit("daemon", json!({"on": false}));
        tray::refresh(app);
        Ok("off".into())
    }
}

#[tauri::command]
async fn set_daemon(app: tauri::AppHandle, on: bool) -> Result<String, String> {
    power(&app, on).await
}

fn err<T: std::fmt::Display>(e: T) -> String {
    e.to_string()
}

fn user_ctx(cwd: &str) -> CallerCtx {
    CallerCtx { agent: "user".into(), cwd: cwd.to_string(), hop: 0 }
}

fn room_workspace(core: &Core, room: &str) -> Result<String, String> {
    let r = core.vault.room(room).map_err(err)?.ok_or_else(|| format!("unknown room {room}"))?;
    Ok(r["workspacePath"].as_str().unwrap_or(".").to_string())
}

/// Cheap snapshot polled by the UI: daemon status, providers, rooms.
#[tauri::command]
fn get_overview(state: State<'_, AppState>) -> Result<Value, String> {
    let core = &state.core;
    let mut rooms = vec![];
    for id in core.vault.room_ids().map_err(err)? {
        if let Some(r) = core.vault.room(&id).map_err(err)? {
            rooms.push(r);
        }
    }
    Ok(json!({
        "daemon": state.daemon.lock().unwrap().clone(),
        "socket": config::socket_path().to_string_lossy(),
        "home": config::home().to_string_lossy(),
        "providers": core.vault.providers().map_err(err)?,
        "rooms": rooms,
    }))
}

/// Per-provider app/CLI detection and registration state. Shells out, so it is on-demand only.
#[tauri::command]
async fn detect_providers() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let v: Vec<Value> = config::AGENTS.iter().map(|(id, _)| connectors::detect(id)).collect();
        json!(v)
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn connect_provider(id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || connectors::install(&id).map_err(err)).await.map_err(err)?
}

#[tauri::command]
async fn disconnect_provider(id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || connectors::uninstall(&id).map_err(err)).await.map_err(err)?
}

#[tauri::command]
fn get_messages(state: State<'_, AppState>, session_id: String) -> Result<Value, String> {
    Ok(json!(state.core.vault.messages(&session_id).map_err(err)?))
}

/// Send a message as the human user to an agent; the reply arrives as a `mux://event`.
#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    room: String,
    session: Option<String>,
    to: String,
    text: String,
    then: Option<String>,
) -> Result<String, String> {
    let ws = room_workspace(&state.core, &room)?;
    let then = then.filter(|t| !t.is_empty() && t != "none");
    // The UI sends into the session you're looking at.
    state.core.start_ticket_in(&user_ctx(&ws), Some(&room), session.as_deref(), &to, &text, then).map_err(err)
}

/// Full wake -> reply round trip against one provider, in a scratch room.
#[tauri::command]
async fn ping_provider(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let dir = config::home().join("selftest");
    std::fs::create_dir_all(&dir).map_err(err)?;
    state
        .core
        .start_ticket(
            &user_ctx(&dir.to_string_lossy()),
            None,
            &id,
            "AgentMux connection test. Reply with exactly the word PONG.",
            None,
        )
        .map_err(err)
}

#[tauri::command]
fn create_room(state: State<'_, AppState>, path: String, members: Option<Vec<String>>) -> Result<String, String> {
    let p = Path::new(&path);
    if !p.is_dir() {
        return Err(format!("{path} is not a directory"));
    }
    let root = context::workspace_root(p);
    let (branch, diff) = context::capture_git(&root);
    let (room, _) = state.core.vault.ensure_room(&root.to_string_lossy(), &branch).map_err(err)?;
    if !diff.is_empty() {
        state.core.vault.set_context(&room, "diff", &diff).map_err(err)?;
    }
    for m in members.unwrap_or_default() {
        if state.core.vault.is_agent(&m) {
            state.core.vault.add_member(&room, &m).map_err(err)?;
        }
    }
    let _ = state.core.vault.export_markdown(&room);
    state.core.emit("room", json!({"id": room}));
    Ok(room)
}

#[tauri::command]
fn new_session(state: State<'_, AppState>, room: String, title: String) -> Result<String, String> {
    let id = state.core.vault.new_session(&room, &title).map_err(err)?;
    state.core.emit("room", json!({"id": room}));
    Ok(id)
}

#[tauri::command]
fn set_context(state: State<'_, AppState>, room: String, kind: String, content: String) -> Result<(), String> {
    if !matches!(kind.as_str(), "notes" | "terminal" | "diff") {
        return Err("kind must be notes, terminal or diff".into());
    }
    state.core.vault.set_context(&room, &kind, &content).map_err(err)?;
    let _ = state.core.vault.export_markdown(&room);
    state.core.emit("context", json!({"room": room, "kind": kind}));
    Ok(())
}

/// Re-read the live `git diff HEAD` for a room's workspace.
#[tauri::command]
fn refresh_git(state: State<'_, AppState>, room: String) -> Result<(), String> {
    let ws = room_workspace(&state.core, &room)?;
    let (branch, diff) = context::capture_git(Path::new(&ws));
    state.core.vault.set_context(&room, "diff", &diff).map_err(err)?;
    state.core.vault.ensure_room(&ws, &branch).map_err(err)?;
    state.core.emit("context", json!({"room": room, "kind": "diff"}));
    Ok(())
}

#[tauri::command]
fn set_allow_edits(state: State<'_, AppState>, room: String, allow: bool) -> Result<(), String> {
    state.core.vault.set_allow_edits(&room, allow).map_err(err)?;
    state.core.emit("room", json!({"id": room}));
    Ok(())
}

#[tauri::command]
fn search(state: State<'_, AppState>, query: String) -> Result<Value, String> {
    Ok(json!(state.core.vault.search(&query).map_err(err)?))
}

#[tauri::command]
fn vault_files() -> Result<Value, String> {
    let dir = config::threads_dir();
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                let meta = e.metadata().map_err(err)?;
                out.push(json!({
                    "filename": p.file_name().unwrap().to_string_lossy(),
                    "diskPath": p.to_string_lossy(),
                    "sizeBytes": meta.len(),
                    "content": std::fs::read_to_string(&p).unwrap_or_default(),
                }));
            }
        }
    }
    Ok(json!(out))
}

#[tauri::command]
fn save_vault_file(filename: String, content: String) -> Result<(), String> {
    let name = Path::new(&filename).file_name().ok_or("bad filename")?.to_os_string();
    let path = config::threads_dir().join(name);
    if path.extension().map(|x| x != "md").unwrap_or(true) {
        return Err("only .md files can be saved".into());
    }
    vault::atomic_write(&path, &content).map_err(err)
}

/// Raw JSON-RPC against the in-process MCP handler (the MCP Inspector).
#[tauri::command]
async fn mcp_rpc(state: State<'_, AppState>, request: Value, cwd: Option<String>) -> Result<Value, String> {
    let cwd = cwd.unwrap_or_else(|| config::home().to_string_lossy().to_string());
    Ok(mcp::handle(&state.core, &user_ctx(&cwd), request).await.unwrap_or(Value::Null))
}

fn bot(agent: Option<String>) -> String {
    agent.filter(|a| !a.is_empty()).unwrap_or_else(|| "grok".into())
}

#[tauri::command]
fn grok_webhook_status(state: State<'_, AppState>, agent: Option<String>) -> Value {
    webhook::status(&state.core.vault, &bot(agent))
}

/// Validates the URL, puts the key in the macOS Keychain and switches the bot to webhook wake.
#[tauri::command]
async fn grok_webhook_save(state: State<'_, AppState>, agent: Option<String>, url: String, key: String) -> Result<Value, String> {
    let vault = state.core.vault.clone();
    let agent = bot(agent);
    tauri::async_runtime::spawn_blocking(move || {
        webhook::save(&vault, &agent, &url, &key).map_err(err)?;
        Ok::<Value, String>(webhook::status(&vault, &agent))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn grok_webhook_clear(state: State<'_, AppState>, agent: Option<String>) -> Result<Value, String> {
    let vault = state.core.vault.clone();
    let agent = bot(agent);
    tauri::async_runtime::spawn_blocking(move || {
        webhook::clear(&vault, &agent).map_err(err)?;
        Ok::<Value, String>(webhook::status(&vault, &agent))
    })
    .await
    .map_err(err)?
}

/// Fires the bot's routine once with a no-op context. Starts one real Grok run.
#[tauri::command]
async fn grok_webhook_test(state: State<'_, AppState>, agent: Option<String>) -> Result<String, String> {
    let vault = state.core.vault.clone();
    let agent = bot(agent);
    let (url, key) = tauri::async_runtime::spawn_blocking(move || webhook::load(&vault, &agent).map_err(err)).await.map_err(err)??;
    let run = webhook::post(&url, &key, "AgentMux webhook test. No action is needed; you can end this run.")
        .await
        .map_err(err)?;
    Ok(run.map(|r| format!("Webhook accepted (run {r})")).unwrap_or_else(|| "Webhook accepted".into()))
}

/// Another Grok bot contact: its own id, token, webhook and inbox.
#[tauri::command]
fn add_bot(state: State<'_, AppState>, name: String) -> Result<String, String> {
    let id = state.core.vault.add_bot(&name).map_err(err)?;
    state.core.emit("provider_connected", json!({"id": id}));
    Ok(id)
}

#[tauri::command]
async fn remove_bot(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let (vault, gw) = (state.core.vault.clone(), state.gateway.clone());
    let core = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || {
        vault.remove_bot(&id).map_err(err)?;
        let _ = webhook::clear(&vault, &id);
        gw.forget(&id);
        core.emit("provider_connected", json!({"id": id}));
        Ok::<(), String>(())
    })
    .await
    .map_err(err)?
}

/// Let a Grok bot start conversations with other agents over the tunnel (off by default).
#[tauri::command]
fn set_bot_can_ask(state: State<'_, AppState>, id: String, on: bool) -> Result<(), String> {
    state.core.vault.set_can_ask(&id, on).map_err(err)?;
    state.core.emit("provider_connected", json!({"id": id}));
    Ok(())
}

#[tauri::command]
fn rename_agent(state: State<'_, AppState>, id: String, name: String, description: Option<String>) -> Result<(), String> {
    state.core.vault.set_profile(&id, &name, description.as_deref()).map_err(err)?;
    state.core.emit("provider_connected", json!({"id": id}));
    Ok(())
}

#[tauri::command]
fn gateway_status(state: State<'_, AppState>) -> Value {
    state.gateway.status()
}

#[tauri::command]
async fn gateway_start(state: State<'_, AppState>) -> Result<Value, String> {
    let gw = state.gateway.clone();
    gw.start().await.map_err(err)?;
    Ok(gw.status())
}

#[tauri::command]
async fn gateway_stop(state: State<'_, AppState>) -> Result<Value, String> {
    state.gateway.stop().await;
    Ok(state.gateway.status())
}

#[tauri::command]
fn gateway_set_provider(state: State<'_, AppState>, provider: String, ngrok_domain: Option<String>) -> Result<Value, String> {
    state.gateway.set_provider(&provider, ngrok_domain.as_deref()).map_err(err)?;
    Ok(state.gateway.status())
}

#[tauri::command]
async fn tunnel_cf_login(state: State<'_, AppState>) -> Result<Value, String> {
    state.gateway.cf_login().await.map_err(err)?;
    Ok(state.gateway.status())
}

/// One click: create the permanent tunnel, then swap it in for any temporary one and start it.
#[tauri::command]
async fn tunnel_cf_create(state: State<'_, AppState>, hostname: String) -> Result<Value, String> {
    let gw = state.gateway.clone();
    // Create first: if that fails, the temporary tunnel Grok uses keeps running.
    gw.cf_create(&hostname).await.map_err(err)?;
    gw.stop().await;
    gw.start().await.map_err(err)?;
    Ok(gw.status())
}

#[tauri::command]
async fn tunnel_cf_switch_domain(state: State<'_, AppState>) -> Result<Value, String> {
    state.gateway.cf_switch_domain().await.map_err(err)?;
    Ok(state.gateway.status())
}

#[tauri::command]
async fn tunnel_verify(state: State<'_, AppState>) -> Result<String, String> {
    state.gateway.verify().await.map_err(err)
}

#[tauri::command]
async fn tunnel_cf_token(state: State<'_, AppState>, token: String, hostname: String) -> Result<Value, String> {
    let gw = state.gateway.clone();
    tauri::async_runtime::spawn_blocking(move || gw.cf_use_token(&token, &hostname).map(|_| gw.status()).map_err(err))
        .await
        .map_err(err)?
}

#[tauri::command]
fn tunnel_cf_reset(state: State<'_, AppState>) -> Result<Value, String> {
    state.gateway.cf_reset().map_err(err)?;
    Ok(state.gateway.status())
}

#[tauri::command]
async fn tunnel_ngrok_setup(state: State<'_, AppState>, authtoken: Option<String>, domain: Option<String>) -> Result<Value, String> {
    let gw = state.gateway.clone();
    tauri::async_runtime::spawn_blocking(move || gw.ngrok_setup(authtoken.as_deref(), domain.as_deref()).map(|_| gw.status()).map_err(err))
        .await
        .map_err(err)?
}

#[tauri::command]
fn tunnel_set_autostart(state: State<'_, AppState>, on: bool) -> Result<Value, String> {
    state.gateway.set_autostart(on).map_err(err)?;
    Ok(state.gateway.status())
}

/// The URL and bearer token to paste into Grok's connector. The only command that returns the token.
#[tauri::command]
async fn gateway_reveal(state: State<'_, AppState>, agent: Option<String>) -> Result<Value, String> {
    let gw = state.gateway.clone();
    let agent = bot(agent);
    tauri::async_runtime::spawn_blocking(move || {
        let token = gw.token(&agent).map_err(err)?;
        Ok::<Value, String>(json!({"token": token, "mcpUrl": gw.status()["mcpUrl"]}))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn gateway_rotate_token(state: State<'_, AppState>, agent: Option<String>) -> Result<Value, String> {
    let gw = state.gateway.clone();
    let agent = bot(agent);
    tauri::async_runtime::spawn_blocking(move || {
        let token = gw.rotate_token(&agent).map_err(err)?;
        Ok::<Value, String>(json!({"token": token, "mcpUrl": gw.status()["mcpUrl"]}))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn sync_status(state: State<'_, AppState>) -> Value {
    sync::status(&state.core.vault)
}

/// Point sync at a private repo and run the first sync.
#[tauri::command]
async fn sync_connect(state: State<'_, AppState>, remote: String, include_diffs: bool, auto: bool) -> Result<Value, String> {
    let v = state.core.vault.clone();
    sync::connect(&v, &remote, include_diffs, auto).await.map_err(err)?;
    let report = sync::sync_now(&v).await.map_err(err)?;
    state.core.emit("room", json!({"sync": true}));
    Ok(json!({"status": sync::status(&v), "report": report}))
}

#[tauri::command]
async fn sync_now(state: State<'_, AppState>) -> Result<Value, String> {
    let v = state.core.vault.clone();
    let result = sync::sync_now(&v).await;
    state.core.emit("room", json!({"sync": true}));
    let report = result.map_err(err)?;
    Ok(json!({"status": sync::status(&v), "report": report}))
}

#[tauri::command]
fn sync_set_options(state: State<'_, AppState>, include_diffs: bool, auto: bool) -> Result<Value, String> {
    sync::set_options(&state.core.vault, include_diffs, auto).map_err(err)?;
    Ok(sync::status(&state.core.vault))
}

#[tauri::command]
fn sync_disconnect(state: State<'_, AppState>) -> Result<Value, String> {
    sync::disconnect(&state.core.vault).map_err(err)
}

const AUTO_UPDATE: &str = "updates.auto";

fn auto_update_on(core: &Core) -> bool {
    core.vault.setting(AUTO_UPDATE).ok().flatten().as_deref() != Some("0")
}

async fn find_update(app: &tauri::AppHandle) -> Result<Option<tauri_plugin_updater::Update>, String> {
    app.updater().map_err(err)?.check().await.map_err(err)
}

fn update_json(u: &Option<tauri_plugin_updater::Update>) -> Value {
    match u {
        Some(u) => json!({"available": true, "version": u.version, "notes": u.body}),
        None => json!({"available": false}),
    }
}

#[tauri::command]
fn app_version(app: tauri::AppHandle, state: State<'_, AppState>) -> Value {
    json!({"version": app.package_info().version.to_string(), "autoCheck": auto_update_on(&state.core)})
}

#[tauri::command]
fn set_auto_update(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    state.core.vault.set_setting(AUTO_UPDATE, if on { "1" } else { "0" }).map_err(err)
}

/// Manual check from Settings: errors are shown to the user.
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Value, String> {
    let u = find_update(&app).await?;
    *app.state::<AppState>().update.lock().unwrap() = u.as_ref().map(|x| x.version.clone());
    tray::refresh(&app);
    Ok(update_json(&u))
}

/// Download, verify (signature against the public key in tauri.conf.json), install, relaunch.
#[tauri::command]
async fn install_update(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let Some(update) = find_update(&app).await? else { return Err("AgentMux is already up to date".into()) };
    // An update only replaces the app bundle; ~/.agentmux and the Keychain are untouched. Still,
    // snapshot the vault first, and don't update at all if that fails.
    let label = format!("v{}", app.package_info().version);
    let vault = state.core.vault.clone();
    tauri::async_runtime::spawn_blocking(move || vault.backup(&config::backups_dir(), &label, 5))
        .await
        .map_err(err)?
        .map_err(|e| format!("Update cancelled: couldn't back up your vault first ({e})"))?;
    let mut done: u64 = 0;
    let handle = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                let _ = handle.emit("update://progress", json!({"done": done, "total": total}));
            },
            || {},
        )
        .await
        .map_err(err)?;
    app.restart()
}

pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            // Closing the window hides it; the daemon keeps serving agents from the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let core = Core::new()?;
            let paused = config::is_paused();
            let status = Arc::new(Mutex::new(if paused { "off" } else { "embedded" }.to_string()));
            let switch = daemon::Switch::new(core.clone());

            let gateway = Gateway::new(core.clone());
            if paused {
                gateway.set_paused(true); // turned off last time: stay off until the user turns it on
            } else {
                let (sw, st) = (switch.clone(), status.clone());
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = sw.start().await {
                        let m = e.to_string();
                        *st.lock().unwrap() = if m.contains("already running") { "external".into() } else { format!("error: {m}") };
                    }
                });
            }
            let gw = gateway.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = gw.serve_http().await {
                    tracing::warn!("http gateway unavailable: {e}");
                }
            });
            // A tunnel from before a restart keeps serving Grok's existing connector URL.
            let gw = gateway.clone();
            tauri::async_runtime::spawn(async move {
                // Give the HTTP endpoint a moment to bind before a fresh start needs it.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let adopted = gw.adopt().await;
                if gw.paused() {
                    gw.stop().await; // off means off: don't leave an old tunnel exposed
                } else if !adopted && gw.autostart() && gw.permanent() {
                    if let Err(e) = gw.start().await {
                        tracing::warn!("could not auto-start the tunnel: {e}");
                    }
                }
            });

            let mut rx = core.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tokio::sync::broadcast::error::RecvError;
                loop {
                    match rx.recv().await {
                        Ok(v) => {
                            let _ = handle.emit("mux://event", v);
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });

            if let Err(e) = connectors::ensure_bin_link() {
                tracing::warn!("could not create ~/.agentmux/bin/agentmux link: {e}");
            }

            let update_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            // Quiet background check: ~10 s after launch, then every 6 h. Errors stay silent.
            let (handle, core2, slot) = (app.handle().clone(), core.clone(), update_slot.clone());
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                loop {
                    if auto_update_on(&core2) {
                        if let Ok(Some(u)) = find_update(&handle).await {
                            *slot.lock().unwrap() = Some(u.version.clone());
                            let _ = handle.emit("update://available", json!({"version": u.version, "notes": u.body}));
                            tray::refresh(&handle);
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                }
            });

            // Close questions whose wake never started or died with an earlier run of the app.
            let core4 = core.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                loop {
                    core4.sweep_stale();
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            });

            // Automatic sync: about 2 minutes after launch, then every 15 minutes, only when switched on.
            let core3 = core.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                loop {
                    let on = core3.vault.setting("sync.auto").ok().flatten().as_deref() == Some("1") && core3.vault.setting("sync.remote").ok().flatten().is_some();
                    if on && !config::is_paused() {
                        if let Err(e) = sync::sync_now(&core3.vault).await {
                            tracing::warn!("auto sync failed: {e}");
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
                }
            });

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("AgentMux")
                .on_menu_event(|app, ev| tray::on_event(app, ev.id.as_ref()))
                .build(app)?;

            app.manage(AppState { core: core.clone(), gateway, daemon: status, switch, update: update_slot, tray_sig: Mutex::new(None) });

            // Keep the menu current: on any event (debounced), and at least every 30 s so "5m ago" stays true.
            let handle = app.handle().clone();
            let mut rx = core.subscribe();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                loop {
                    tray::refresh(&handle);
                    tokio::select! {
                        _ = rx.recv() => { tokio::time::sleep(std::time::Duration::from_millis(500)).await; while rx.try_recv().is_ok() {} }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_overview,
            detect_providers,
            connect_provider,
            disconnect_provider,
            get_messages,
            send_message,
            ping_provider,
            create_room,
            new_session,
            set_context,
            refresh_git,
            set_allow_edits,
            search,
            vault_files,
            save_vault_file,
            mcp_rpc,
            grok_webhook_status,
            grok_webhook_save,
            grok_webhook_clear,
            grok_webhook_test,
            add_bot,
            remove_bot,
            rename_agent,
            set_bot_can_ask,
            sync_status,
            sync_connect,
            sync_now,
            sync_set_options,
            sync_disconnect,
            gateway_status,
            gateway_start,
            gateway_stop,
            gateway_set_provider,
            gateway_reveal,
            tunnel_cf_login,
            tunnel_cf_create,
            tunnel_cf_switch_domain,
            tunnel_verify,
            tunnel_cf_token,
            tunnel_cf_reset,
            tunnel_ngrok_setup,
            tunnel_set_autostart,
            app_version,
            set_daemon,
            set_auto_update,
            check_update,
            install_update,
            gateway_rotate_token,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AgentMux");
}
