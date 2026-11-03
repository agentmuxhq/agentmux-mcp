# Adding a provider

A provider is one Rust file. You don't need to touch the daemon, the MCP tools, the gateway or the UI. They all read the provider's `info()` and `caps()`.

For a CLI agent that just needs "register MCP, run headless, read stdout", try a [TOML manifest](./custom-agents.md) first. It needs no Rust.

## 1. Copy a template

`providers/codex.rs` is the simplest CLI provider; `providers/cursor.rs` shows a JSON-config one.

```sh
cp crates/agentmux-core/src/providers/codex.rs crates/agentmux-core/src/providers/myagent.rs
```

## 2. Implement the trait

```rust
use super::*;
use crate::wakers::find_bin;

pub struct MyAgent;

impl Provider for MyAgent {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "myagent".into(), name: "My Agent".into(), color: "#E11D48".into(),
                       hint: "MCP stdio · ~/.myagent/mcp.json".into() }
    }
    fn caps(&self) -> Capabilities {
        Capabilities { multi_instance: false, remote_access: false, wake: "cli", context_hook: false }
    }
    fn detect(&self) -> (bool, String) {
        match find_bin("myagent") {
            Some(b) => (true, b.display().to_string()),
            None => (false, "myagent CLI not found".into()),
        }
    }
    fn configured(&self) -> bool { /* read its config, look for "agentmux" */ false }
    fn install(&self, link: &Path) -> Result<String> {
        let entry = json!({"command": link.to_string_lossy(), "args": ["mcp", "--agent", "myagent"]});
        json_merge_server(&config_path(), "mcpServers.agentmux", entry, "Restart My Agent to load it.")
    }
    fn uninstall(&self) -> Result<String> { /* json_remove_server(...) */ Ok("Removed".into()) }
    fn wake(&self, r: &WakeRequest) -> Option<WakeSpec> {
        let mut args = vec!["--print".to_string(), r.prompt.to_string()];
        if !r.allow_edits { args.push("--read-only".into()); }
        Some(WakeSpec { bin: find_bin("myagent")?, args })
    }
}
```

**Rules:**
- **Read-only by default.** When `allow_edits` is false, the wake must not be able to change files.
- **Never print a user's config** in messages; it may contain other servers' tokens. Back it up before editing (the `json_merge_server` helper does this).
- **No shells.** Build an argv list; never use `sh -c`.
- **Secrets go through `secrets.rs`** (macOS Keychain), never the vault or logs.
- **Ids are permanent.** They're stored in users' vaults and configs.

## 3. Register it

Add it to `builtin()` in `providers/mod.rs`:

```rust
pub mod myagent;
// …
vec![Arc::new(cursor::Cursor), …, Arc::new(myagent::MyAgent)]
```

The provider now appears in `agentmux provider list`, in `connect all`, in `init` (for `cli` wakes), in the app's Connect screen, and in `mux_contacts`.

## 4. Optional: special powers

| You want | Do |
|---|---|
| Many named instances | `multi_instance: true`; ids become `myagent:<slug>` |
| Cloud access through the tunnel | `remote_access: true`; it gets per-instance tokens and the restricted remote tool set. Keep its permissions as narrow as Grok's ([Grok bots](./grok.md#what-a-bots-token-allows)) |
| Show replies in its own chat | `context_hook: true`, plus `set_option("hook", …)` and `options()`; see `claude_code.rs` and `crates/agentmux-cli/src/hook.rs` |
| A webhook wake | `wake: "webhook"`; see how `core::wake_webhook` and `webhook.rs` handle Grok |

## 5. Test it

Add a test to `crates/agentmux-core/tests/providers.rs` that puts a fake binary on `PATH` and checks your exact wake arguments, read-only and with edits. The `registry_and_golden_wake_args` test shows how. For an end-to-end wake, follow `tests/manifest.rs`: a fake CLI answers a real ticket.

```sh
cargo test -p agentmux-core
```

## 6. Document it and open a PR

- Add `docs/myagent.md` (copy `docs/codex.md`) and an entry in `docs/nav.json` and `docs/providers.md`.
- Open a PR using the **New provider** template: what the agent is, how it's detected, its wake command, and how you verified read-only mode.
