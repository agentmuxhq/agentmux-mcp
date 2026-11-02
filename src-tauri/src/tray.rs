//! The menu bar menu: on/off switch, tunnel, providers, sync and updates at a glance.
//!
//! The menu is described as plain data first (`entries`), compared with the last one shown, and only
//! rebuilt when something changed, so it stays cheap to refresh on every event.

use crate::{power, show_main, AppState};
use agentmux_core::sync;
use serde_json::json;
use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum E {
    Item { id: &'static str, text: String, enabled: bool, acc: Option<&'static str> },
    Check { id: &'static str, text: String, on: bool },
    Sep,
    Sub { text: String, items: Vec<E> },
}

fn item(id: &'static str, text: impl Into<String>) -> E {
    E::Item { id, text: text.into(), enabled: true, acc: None }
}

/// A greyed-out line of information.
fn info(text: impl Into<String>) -> E {
    E::Item { id: "info", text: text.into(), enabled: false, acc: None }
}

fn ago(iso: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso) else { return "never".into() };
    let s = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

fn entries(app: &AppHandle) -> Vec<E> {
    let st = app.state::<AppState>();
    let daemon = st.daemon.lock().unwrap().clone();
    let on = daemon == "embedded" || daemon == "external";
    let gw = st.gateway.status();
    let sy = sync::status(&st.core.vault);
    let providers = st.core.vault.providers().unwrap_or_default();
    let version = app.package_info().version.to_string();

    let mut v = vec![
        E::Check { id: "toggle_daemon", text: if on { "AgentMux is on" } else { "AgentMux is off" }.into(), on },
        info(match daemon.as_str() {
            "embedded" => "Local daemon · running".to_string(),
            "external" => "Local daemon · running in another process".to_string(),
            "off" => "Off: agents can't reach AgentMux".to_string(),
            other => other.replace("error: ", "Error: "),
        }),
        E::Sep,
    ];

    // Public access (the tunnel).
    let state = gw["state"].as_str().unwrap_or("stopped");
    let live = state == "running" || state == "starting";
    let label = match state {
        "running" => "online",
        "starting" => "starting…",
        "error" => "error",
        _ => "off",
    };
    let host = gw["url"].as_str().map(|u| u.trim_start_matches("https://").to_string()).or_else(|| gw["cfHostname"].as_str().map(String::from)).or_else(|| gw["ngrokDomain"].as_str().map(String::from));
    let kind = if gw["provider"] == "ngrok" { "ngrok" } else { "Cloudflare" };
    let mode = if gw["permanent"] == true { if gw["verified"] == true { "permanent · verified" } else { "permanent · not verified" } } else { "temporary" };
    let mut pa = vec![info(format!("{kind} tunnel · {mode}"))];
    if let Some(h) = host.filter(|_| live || gw["permanent"] == true) {
        pa.push(info(h));
    }
    if let Some(e) = gw["error"].as_str() {
        pa.push(info(format!("Error: {}", e.chars().take(60).collect::<String>())));
    }
    pa.push(E::Sep);
    pa.push(E::Item { id: "copy_mcp", text: "Copy MCP URL".into(), enabled: gw["mcpUrl"].is_string(), acc: None });
    pa.push(E::Item { id: "tunnel_toggle", text: if live { "Stop tunnel" } else { "Start tunnel" }.into(), enabled: on, acc: None });
    pa.push(item("settings_access", "Tunnel settings…"));
    v.push(E::Sub { text: format!("Public access · {label}"), items: pa });

    // Providers.
    let connected = providers.iter().filter(|p| p["connectedAt"].is_string()).count();
    let mut pr: Vec<E> = providers
        .iter()
        .map(|p| {
            let up = p["connectedAt"].is_string();
            let seen = p["lastSeen"].as_str().map(|s| format!(" · seen {}", ago(s))).unwrap_or_default();
            info(format!("{} {}{}", if up { "●" } else { "○" }, p["name"].as_str().unwrap_or("?"), if up { seen } else { " · not connected".into() }))
        })
        .collect();
    pr.push(E::Sep);
    pr.push(item("connect", "Connect a provider…"));
    v.push(E::Sub { text: format!("Providers · {connected} connected"), items: pr });

    // Sync.
    let configured = sy["configured"] == true;
    let sync_label = match (configured, sy["last"].as_str()) {
        (false, _) => "not set up".to_string(),
        (true, Some(t)) => format!("synced {}", ago(t)),
        (true, None) => "not synced yet".to_string(),
    };
    let mut sv = vec![info(if configured { sy["remote"].as_str().unwrap_or("").to_string() } else { "Back up to your own private git repo".to_string() })];
    if let Some(e) = sy["error"].as_str() {
        sv.push(info(format!("Error: {}", e.chars().take(60).collect::<String>())));
    }
    sv.push(E::Sep);
    sv.push(E::Item { id: "sync_now", text: "Sync now".into(), enabled: configured, acc: None });
    sv.push(item("settings_sync", if configured { "Sync settings…" } else { "Set up sync…" }));
    v.push(E::Sub { text: format!("Sync · {sync_label}"), items: sv });

    v.push(E::Sep);
    v.push(info(format!("AgentMux v{version}")));
    if let Some(u) = st.update.lock().unwrap().clone() {
        v.push(item("update", format!("Update to v{}…", u.trim_start_matches('v'))));
    }
    v.push(E::Sep);
    v.push(item("open", "Open AgentMux"));
    v.push(E::Item { id: "settings", text: "Settings…".into(), enabled: true, acc: Some("CmdOrCtrl+,") });
    v.push(E::Sep);
    v.push(E::Item { id: "quit", text: "Quit AgentMux".into(), enabled: true, acc: Some("CmdOrCtrl+Q") });
    v
}

fn build(app: &AppHandle, es: &[E]) -> tauri::Result<Menu<Wry>> {
    let mut b = MenuBuilder::new(app);
    for e in es {
        match e {
            E::Item { id, text, enabled, acc } => b = b.item(&MenuItem::with_id(app, *id, text, *enabled, *acc)?),
            E::Check { id, text, on } => b = b.item(&CheckMenuItem::with_id(app, *id, text, true, *on, None::<&str>)?),
            E::Sep => b = b.separator(),
            E::Sub { text, items } => {
                let mut sb = SubmenuBuilder::new(app, text);
                for i in items {
                    match i {
                        E::Item { id, text, enabled, acc } => sb = sb.item(&MenuItem::with_id(app, *id, text, *enabled, *acc)?),
                        E::Sep => sb = sb.separator(),
                        _ => {}
                    }
                }
                b = b.item(&sb.build()?);
            }
        }
    }
    b.build()
}

/// Rebuild the menu if (and only if) what it shows has changed.
pub fn refresh(app: &AppHandle) {
    let es = entries(app);
    let st = app.state::<AppState>();
    {
        let mut last = st.tray_sig.lock().unwrap();
        if *last == Some(es.clone()) {
            return;
        }
        *last = Some(es.clone());
    }
    let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), build(app, &es)) else { return };
    let _ = tray.set_menu(Some(menu));
    let on = matches!(st.daemon.lock().unwrap().as_str(), "embedded" | "external");
    let _ = tray.set_tooltip(Some(if on { "AgentMux · on" } else { "AgentMux · off" }));
}

fn open_settings(app: &AppHandle, tab: &str) {
    show_main(app);
    let _ = app.emit("ui://settings", json!(tab));
}

/// A menu item was clicked.
pub fn on_event(app: &AppHandle, id: &str) {
    let st = app.state::<AppState>();
    let on = matches!(st.daemon.lock().unwrap().as_str(), "embedded" | "external");
    let app2 = app.clone();
    match id {
        "toggle_daemon" => {
            tauri::async_runtime::spawn(async move {
                let _ = power(&app2, !on).await;
            });
        }
        "tunnel_toggle" => {
            let gw = st.gateway.clone();
            tauri::async_runtime::spawn(async move {
                let live = matches!(gw.status()["state"].as_str(), Some("running") | Some("starting"));
                if live {
                    gw.stop().await;
                } else if let Err(e) = gw.start().await {
                    let _ = app2.emit("ui://error", json!(e.to_string()));
                    open_settings(&app2, "access");
                }
                refresh(&app2);
            });
        }
        "copy_mcp" => {
            if let Some(url) = st.gateway.status()["mcpUrl"].as_str() {
                copy_to_clipboard(url);
            }
        }
        "sync_now" => {
            let core = st.core.clone();
            tauri::async_runtime::spawn(async move {
                let _ = sync::sync_now(&core.vault).await;
                refresh(&app2);
            });
        }
        "settings_access" => open_settings(app, "access"),
        "settings_sync" => open_settings(app, "sync"),
        "settings" => open_settings(app, "daemon"),
        "connect" => {
            show_main(app);
            let _ = app.emit("ui://connect", json!(null));
        }
        "update" | "open" => show_main(app),
        "quit" => {
            // Don't leave a public tunnel running after the app is gone.
            let gw = st.gateway.clone();
            tauri::async_runtime::block_on(gw.stop());
            app.exit(0);
        }
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    if let Ok(mut c) = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut i) = c.stdin.take() {
            let _ = i.write_all(text.as_bytes());
        }
        let _ = c.wait();
    }
}

#[cfg(not(target_os = "macos"))]
fn copy_to_clipboard(_text: &str) {}
