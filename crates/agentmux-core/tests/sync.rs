//! Git sync between two "Macs" through a local bare repo (no network).

use agentmux_core::sync;
use agentmux_core::Vault;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success();
    assert!(ok, "git {args:?} failed");
}

fn all_text(dir: &Path) -> String {
    let mut out = String::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(t) = std::fs::read_to_string(&p) {
                out.push_str(&t);
            }
        }
    }
    out
}

/// Switch the process to "Mac" `name`: its own AGENTMUX_HOME (sync clone) and vault.
fn mac(base: &Path, name: &str) -> Arc<Vault> {
    let home = base.join(name);
    std::fs::create_dir_all(&home).unwrap();
    // Safe: this file has a single test, so nothing else reads the environment concurrently.
    unsafe { std::env::set_var("AGENTMUX_HOME", &home) };
    Arc::new(Vault::open_at(&home.join("vault.db"), &home.join("threads")).unwrap())
}

#[test]
fn remotes_are_validated_and_secrets_redacted() {
    for bad in [
        "",
        "https://user:ghp_abcdefabcdefabcdefabcdef@github.com/a/b.git",
        "http://github.com/a/b.git",
        "ext::sh -c touch /tmp/pwned",
        "-oProxyCommand=evil",
        "git@github.com:a/b.git; rm -rf /",
        "just-a-name",
        "https://github.com",
    ] {
        assert!(sync::validate_remote(bad).is_err(), "should reject {bad:?}");
    }
    for ok in ["git@github.com:me/backup.git", "https://github.com/me/backup.git", "ssh://git@host/me/backup.git", "/Volumes/Backup/agentmux.git"] {
        assert!(sync::validate_remote(ok).is_ok(), "should accept {ok}");
    }
    assert_eq!(sync::host_and_path("git@github.com:Me/backup.git"), Some(("github.com".into(), "Me/backup".into())));
    assert_eq!(sync::host_and_path("https://gitlab.com/g/sub/backup.git"), Some(("gitlab.com".into(), "g/sub/backup".into())));
    assert_eq!(sync::host_and_path("/Volumes/x.git"), None);

    let r = sync::redact("token gho_abcdefghijklmnopqrstuvwxyz0123 and sk-abcdefghijklmnopqrstuv, Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456 done");
    assert!(!r.contains("gho_abc") && !r.contains("sk-abc") && !r.contains("abcdefghijklmnopqrstuvwxyz123456"), "{r}");
    assert!(r.contains("token") && r.contains("done"), "{r}");
    let k = sync::redact("before\n-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBg\n-----END PRIVATE KEY-----\nafter");
    assert!(!k.contains("MIIEvQ") && k.contains("before") && k.contains("after"), "{k}");
    assert!(sync::redact("DB_PASSWORD=hunter2hunter2hunter2").contains("[redacted]"));
    assert_eq!(sync::redact("plain words about the deploy, nothing secret (v1.2.3)"), "plain words about the deploy, nothing secret (v1.2.3)");
}

#[tokio::test]
async fn two_macs_share_through_a_git_remote() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sync-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    let remote_s = remote.to_string_lossy().to_string();

    // ── Mac A has some history ──
    let a = mac(&base, "a");
    let bot = a.add_bot("Job Hunter").unwrap();
    let (room, sess) = a.ensure_room("/work/checkout-service", "fix/webhook").unwrap();
    a.add_member(&room, "cursor").unwrap();
    let t = a.create_ticket(&sess, &room, "cursor", "claude", "why is the secret empty?", 1, None).unwrap();
    a.add_message(&sess, &room, "cursor", Some("claude"), "a2a_request", "why is the secret empty? gho_abcdefghijklmnopqrstuvwxyz0123", None, Some(&t)).unwrap();
    a.set_context(&room, "notes", "deploy notes: check env").unwrap();
    a.set_context(&room, "diff", "SECRETDIFF +STRIPE=1").unwrap();
    a.set_setting("tunnel.cf_hostname", "mux.example.com").unwrap();
    a.set_setting(&format!("webhook.{bot}.url"), "https://grok.example.com/automations/webhook/PRIVATE-ROUTINE").unwrap();
    a.export_markdown(&room).unwrap();

    sync::connect(&a, &remote_s, false, false).await.unwrap();
    let rep = sync::sync_now(&a).await.unwrap();
    assert_eq!(rep["pushed"], true, "{rep}");
    let text = all_text(&sync::dir());
    assert!(!text.contains("gho_abcdefghij"), "token must be redacted");
    assert!(text.contains("[redacted]") && text.contains("why is the secret empty?"));
    assert!(!text.contains("SECRETDIFF"), "diffs are off by default");
    assert!(!text.contains("PRIVATE-ROUTINE"), "webhook URLs never sync");
    assert!(text.contains("deploy notes") && text.contains("mux.example.com"));
    let st = sync::status(&a);
    assert_eq!((st["configured"].as_bool(), st["error"].is_null()), (Some(true), true));

    // ── Mac B restores from the same repo ──
    let b = mac(&base, "b");
    sync::connect(&b, &remote_s, false, false).await.unwrap();
    let rep = sync::sync_now(&b).await.unwrap();
    assert_eq!(rep["imported"]["messages"], 1, "{rep}");
    assert!(b.is_agent(&bot));
    let p = b.providers().unwrap().into_iter().find(|p| p["id"] == bot.as_str()).unwrap();
    assert_eq!(p["wakeKind"], "inbox", "a restored bot has no webhook until it's set up again");
    assert!(b.room(&room).unwrap().is_some());
    assert_eq!(b.messages(&sess).unwrap().len(), 1);
    assert_eq!(b.ticket(&t).unwrap().unwrap()["status"], "expired", "questions still open on A must not haunt B");
    assert_eq!(b.setting("tunnel.cf_hostname").unwrap().as_deref(), Some("mux.example.com"));
    assert!(b.setting(&format!("webhook.{bot}.url")).unwrap().is_none());
    assert_eq!(b.context(&room, "notes").unwrap(), "deploy notes: check env");
    assert_eq!(b.context(&room, "diff").unwrap(), "");

    // ── B writes; A picks it up (union, nothing lost on either side) ──
    let s2 = b.new_session(&room, "from B").unwrap();
    b.add_message(&s2, &room, "claude", Some("cursor"), "a2a_reply", "answer written on B", None, None).unwrap();
    sync::sync_now(&b).await.unwrap();
    let local_only = a.new_session(&room, "A offline").unwrap();
    a.add_message(&local_only, &room, "cursor", Some("claude"), "a2a_request", "written on A meanwhile", None, None).unwrap();
    // Back on A's home: sync merges B's session and keeps A's own.
    let a2 = mac(&base, "a");
    let rep = sync::sync_now(&a2).await.unwrap();
    assert!(rep["imported"]["sessions"].as_i64().unwrap() >= 1, "{rep}");
    let all: Vec<Value> = a2.room(&room).unwrap().unwrap()["sessions"].as_array().unwrap().clone();
    assert!(all.len() >= 3, "A should hold its own sessions plus B's: {all:?}");
    assert_eq!(a2.messages(&s2).unwrap()[0]["text"], "answer written on B");

    // ── git diffs travel only when asked, and stop again when switched off ──
    sync::set_options(&a2, true, false).unwrap();
    sync::sync_now(&a2).await.unwrap();
    assert!(all_text(&sync::dir()).contains("SECRETDIFF"));
    sync::set_options(&a2, false, false).unwrap();
    sync::sync_now(&a2).await.unwrap();
    assert!(!all_text(&sync::dir()).contains("SECRETDIFF"));

    // ── errors are reported, and disconnecting forgets everything locally ──
    sync::connect(&a2, &base.join("nope.git").to_string_lossy(), false, false).await.unwrap();
    let err = sync::sync_now(&a2).await.unwrap_err().to_string();
    assert!(err.contains("git fetch failed"), "{err}");
    assert!(sync::status(&a2)["error"].is_string());
    let st = sync::disconnect(&a2).unwrap();
    assert_eq!(st["configured"], false);
    assert!(!sync::dir().join(".git").exists());
}
