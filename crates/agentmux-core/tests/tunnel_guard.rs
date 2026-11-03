//! The permanent tunnel URL can't vanish silently: every change to tunnel settings is logged, the
//! last working config is kept, a cleared one is reported (and blocks a quiet temporary tunnel),
//! and one call restores it. A deliberate "back to temporary" doesn't nag.

use agentmux_core::gateway::Gateway;
use agentmux_core::{Core, Vault};
use std::path::PathBuf;
use std::sync::Arc;

const HOST: &str = "mux.example.com";
const TUNNEL: &str = "0a0829d4-0ad3-4aa6-a3e6-2feed5564065";

#[tokio::test]
async fn cleared_permanent_tunnel_is_reported_and_restorable() {
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tguard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let cf = home.join("cloudflared");
    std::fs::create_dir_all(&cf).unwrap();
    std::fs::write(cf.join("cert.pem"), "cert").unwrap();
    std::fs::write(cf.join(format!("{TUNNEL}.json")), "{}").unwrap();
    // SAFETY: the only test in this binary. The token secret comes from the environment, so the
    // real keychain is never touched.
    unsafe {
        std::env::set_var("TUNNEL_ORIGIN_CERT", cf.join("cert.pem"));
        std::env::set_var("AGENTMUX_SECRET_CLOUDFLARE_TUNNEL_TOKEN", "test-only");
    }

    let vault = Arc::new(Vault::open_at(&home.join("vault.db"), &home.join("threads")).unwrap());
    for (k, v) in [("tunnel.provider", "cloudflared"), ("tunnel.cf_mode", "named"), ("tunnel.cf_tunnel", TUNNEL), ("tunnel.cf_hostname", HOST), ("tunnel.cf_zone", "example.com")] {
        vault.set_setting(k, v).unwrap();
    }
    // A working permanent config is remembered as soon as AgentMux starts.
    let gw = Gateway::new(Core::with_vault(vault.clone()));
    assert!(gw.permanent());
    assert!(gw.lost_permanent().is_none());
    assert!(gw.status()["lostPermanent"].is_null());

    // The incident: the domain, mode and tunnel id disappear; the zone stays.
    for k in ["tunnel.cf_mode", "tunnel.cf_tunnel", "tunnel.cf_hostname"] {
        vault.del_setting(k).unwrap();
    }
    assert!(!gw.permanent());
    assert_eq!(gw.lost_permanent().as_deref(), Some(HOST));
    assert_eq!(gw.status()["lostPermanent"], HOST);
    let log = std::fs::read_to_string(vault.settings_log()).unwrap();
    assert!(log.contains(&format!("delete tunnel.cf_hostname \"{HOST}\"")), "{log}");
    assert!(log.contains("(pid "), "each line says which process changed it: {log}");

    // No silent fallback to a temporary URL.
    let err = gw.start().await.unwrap_err().to_string();
    assert!(err.contains("was cleared") && err.contains(HOST), "{err}");

    // One call brings it back.
    assert_eq!(gw.restore_last_good().unwrap(), HOST);
    assert!(gw.permanent());
    assert!(gw.lost_permanent().is_none());
    assert_eq!(vault.setting("tunnel.cf_tunnel").unwrap().as_deref(), Some(TUNNEL));

    // A deliberate "Switch back to temporary" doesn't warn, but can still be undone.
    gw.cf_reset().unwrap();
    assert!(!gw.permanent());
    assert!(gw.lost_permanent().is_none(), "the user chose this");
    assert_eq!(gw.restore_last_good().unwrap(), HOST);
    assert!(gw.permanent());

    // "Use a temporary tunnel instead" after a wipe silences the warning.
    vault.del_setting("tunnel.cf_hostname").unwrap();
    assert!(gw.lost_permanent().is_some());
    gw.dismiss_lost().unwrap();
    assert!(gw.lost_permanent().is_none());

    // Restoring is refused when the tunnel's Cloudflare credentials are gone.
    std::fs::remove_file(cf.join(format!("{TUNNEL}.json"))).unwrap();
    let err = gw.restore_last_good().unwrap_err().to_string();
    assert!(err.contains("credentials") && err.contains(TUNNEL), "{err}");
}
