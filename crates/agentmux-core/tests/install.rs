//! Where the stable `~/.agentmux/bin/agentmux` link points for each way of installing.

use agentmux_core::connectors::stable_target;
use std::path::{Path, PathBuf};

#[test]
fn npx_copies_brew_uses_the_stable_prefix_and_apps_link_directly() {
    // Safe: single test in this file.
    unsafe { std::env::set_var("AGENTMUX_HOME", "/tmp/amx-install-test") };
    let (t, copy) = stable_target(Path::new("/Users/me/.npm/_npx/1a2b/node_modules/@agentmuxhq/cli-darwin-arm64/bin/agentmux"));
    assert!(copy, "the npx cache gets cleaned, so the binary must be copied");
    assert_eq!(t, PathBuf::from("/tmp/amx-install-test/bin/agentmux-cli"));

    let (t, copy) = stable_target(Path::new("/opt/homebrew/Cellar/agentmux/0.2.5/bin/agentmux"));
    assert!(!copy);
    assert_eq!(t, PathBuf::from("/opt/homebrew/bin/agentmux"), "brew upgrade deletes the Cellar version");

    let (t, copy) = stable_target(Path::new("/usr/local/Cellar/agentmux/0.2.5/bin/agentmux"));
    assert_eq!((t, copy), (PathBuf::from("/usr/local/bin/agentmux"), false));

    let app = Path::new("/Applications/AgentMux.app/Contents/MacOS/agentmux");
    assert_eq!(stable_target(app), (app.to_path_buf(), false));
}
