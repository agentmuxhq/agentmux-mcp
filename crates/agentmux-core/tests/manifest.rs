//! A custom provider defined only by a TOML manifest answers a ticket end to end.

use agentmux_core::{providers, CallerCtx, Core, Vault};
use std::path::PathBuf;

#[tokio::test]
async fn manifest_provider_wakes() {
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("providers")).unwrap();
    std::fs::create_dir_all(home.join("ws")).unwrap();
    let bin = home.join("echo-agent");
    std::fs::write(&bin, "#!/bin/sh\n[ \"$1\" = \"--ask\" ] && echo \"pong from $AGENTMUX_TICKET\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        home.join("providers/echo.toml"),
        r#"id = "echo"
name = "Echo Agent"
detect = { binary = "echo-agent" }
mcp = { kind = "json-merge", path = "/nonexistent/mcp.json", key = "mcpServers.agentmux" }
wake = { binary = "echo-agent", args = ["--ask", "{prompt}"] }
"#,
    )
    .unwrap();
    // SAFETY: only test in this binary.
    unsafe {
        std::env::set_var("AGENTMUX_HOME", &home);
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", home.display()));
    }
    assert_eq!(providers::display_name("echo"), "Echo Agent");
    let vault = std::sync::Arc::new(Vault::open_at(&home.join("vault.db"), &home.join("threads")).unwrap());
    assert!(vault.providers().unwrap().iter().any(|p| p["id"] == "echo" && p["wakeKind"] == "cli"));
    let core = Core::with_vault(vault);
    let ctx = CallerCtx { agent: "claude".into(), cwd: home.join("ws").to_string_lossy().into(), hop: 0 };
    let t = core.start_ticket(&ctx, None, "echo", "ping", None).unwrap();
    let done = core.wait_ticket(&t, 10).await.unwrap();
    assert_eq!(done["status"], "answered", "{done}");
    assert_eq!(done["answer"], format!("pong from {t}"));
}
