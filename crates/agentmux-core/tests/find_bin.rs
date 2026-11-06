//! Agents installed through a Node version manager are found even when AgentMux runs without the
//! shell's PATH (the desktop app, the login service).

use std::os::unix::fs::PermissionsExt;

#[test]
fn finds_clis_installed_by_node_managers() {
    let home = std::env::temp_dir().join(format!("agentmux-findbin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    for (dir, bin) in [
        (".local/share/mise/installs/node/22.22.0/bin", "gemini"),
        (".volta/bin", "codex-fake"),
        (".asdf/installs/nodejs/20.1.0/bin", "asdf-tool"),
    ] {
        let d = home.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(bin);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // SAFETY: the only test in this binary. A login service has a minimal PATH and no shell profile.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("PATH", "/usr/bin:/bin");
    }
    let found = agentmux_core::wakers::find_bin("gemini").expect("mise-installed gemini");
    assert!(found.ends_with(".local/share/mise/installs/node/22.22.0/bin/gemini"), "{found:?}");
    assert!(agentmux_core::wakers::find_bin("codex-fake").is_some());
    assert!(agentmux_core::wakers::find_bin("asdf-tool").is_some());
    assert!(agentmux_core::wakers::spawn_path().contains("mise/installs/node/22.22.0/bin"), "woken CLIs can find node too");
}

#[test]
fn signed_out_wakes_say_how_to_sign_in() {
    use agentmux_core::wakers::login_hint;
    let h = login_hint("claude", "/Users/x/.local/bin/claude", "Failed to authenticate: OAuth session expired and could not be refreshed").unwrap();
    assert!(h.contains("claude auth login") && h.contains("@claude"), "{h}");
    assert!(login_hint("chatgpt", "/opt/homebrew/bin/codex", "Error: not logged in").unwrap().contains("codex login"));
    assert!(login_hint("claude", "/x/claude", "Error: rate limited, try later").is_none());
}
