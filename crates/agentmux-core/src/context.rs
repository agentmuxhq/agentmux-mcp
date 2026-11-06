use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_DIFF: usize = 20_000;

/// Walk up from `start` to the nearest directory containing `.git`.
/// Folders an agent runs in that aren't a project: your home folder, the filesystem root, and the
/// per-chat scratch folders some apps make (Codex: ~/Documents/Codex/<date>/<chat>). Questions from
/// these are filed by what they're about, not by the folder. (AgentMux's own home is the Cloud
/// bots room and is handled separately.)
pub fn is_scratch_dir(root: &Path) -> bool {
    let home = dirs::home_dir().unwrap_or_default();
    // AgentMux's own folder is where cloud bots' requests come from: never a product either.
    root == home || root == Path::new("/") || root.starts_with(home.join("Documents/Codex")) || root.starts_with(crate::config::home())
}

pub fn workspace_root(start: &Path) -> PathBuf {
    let mut cur = start;
    loop {
        if cur.join(".git").exists() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return start.to_path_buf(),
        }
    }
}

fn git(ws: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(ws).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// (branch, diff vs HEAD). Empty strings when `ws` is not a git repo.
pub fn capture_git(ws: &Path) -> (String, String) {
    let branch = git(ws, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default().trim().to_string();
    let mut diff = git(ws, &["diff", "HEAD"]).or_else(|| git(ws, &["diff"])).unwrap_or_default();
    if diff.len() > MAX_DIFF {
        let mut end = MAX_DIFF;
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
        diff.push_str("\n… (diff truncated)");
    }
    (branch, diff)
}
