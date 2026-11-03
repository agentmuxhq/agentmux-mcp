# Architecture

## Processes

- **Shim** (`agentmux mcp --agent <id>`): one per client. Every agent launches it over stdio. It forwards JSON-RPC to the daemon's Unix socket (`~/.agentmux/run/agentmux.sock`, `0600`) after an `agentmux_hello` that says who it is and its working directory.
- **Daemon**: holds the vault and runs tickets. It is either embedded in the desktop app or run headless by the login service (`agentmux daemon`, launchd `dev.agentmux.daemon`). Whichever starts first serves the socket; the app takes over from a headless daemon when it opens. `~/.agentmux/run/paused` is the on/off switch they all respect.
- **HTTP gateway**: `127.0.0.1:4317`, `POST /mcp` with a bearer token, for [remote providers](./grok.md) through a tunnel.

## Core modules (`crates/agentmux-core/src`)

| Module | Responsibility |
|---|---|
| `providers/` | The `Provider` trait, the registry (`all`, `get`) and one file per provider |
| `mcp.rs` | MCP JSON-RPC: tool list and dispatch, optionally scoped to a tool allow-list |
| `core.rs` | Rooms, sessions, tickets, `run_ticket` (picks the wake from the provider), `finish`, handoffs, sweeper |
| `wakers.rs` | `run_wake`: runs a provider's `WakeSpec` with a stripped env and timeout; `find_bin`, `spawn_path` |
| `vault.rs` | SQLite schema and queries, `updates_for`, Markdown export, sync dump/restore |
| `connectors.rs` | Stable binary link, thin `detect`/`install`/`uninstall`/`set_option` over the registry |
| `gateway.rs` | HTTP gateway, per-instance tokens, tunnels (Cloudflare quick/named, ngrok) |
| `webhook.rs` | Webhook config and posting (Grok) |
| `daemon.rs`, `service.rs` | Socket server, control channel, headless service |
| `sync.rs`, `secrets.rs`, `context.rs` | Git backup, Keychain, git diff capture |

## The provider trait

```rust
pub trait Provider: Send + Sync {
    fn info(&self) -> ProviderInfo;          // id, name, color, hint
    fn caps(&self) -> Capabilities;          // wake, multi_instance, remote_access, context_hook
    fn detect(&self) -> (bool, String);      // installed?, detail for the Connect screen
    fn configured(&self) -> bool;            // is AgentMux registered in it?
    fn install(&self, link: &Path) -> Result<String>;
    fn uninstall(&self) -> Result<String>;
    fn wake(&self, req: &WakeRequest) -> Option<WakeSpec> { None }   // headless command, if any
    fn set_option(&self, name: &str, on: bool) -> Result<String>;     // provider-specific switches
    fn options(&self) -> Value;
}
```

**How the rest of AgentMux uses it:**
- `vault` seeds a row per provider, with its default wake kind;
- `core` wakes through `wake()`;
- `gateway` only issues tokens to `remote_access` providers;
- the app gets `providers::catalog()` in its overview and `providers::describe()` on the Connect screen.

None of them match on provider ids.

## A question, end to end

1. Cursor calls `mux_ask_peer {to: "claude", question}`.
2. `core.start_ticket_in` resolves the room (from Cursor's cwd) and the session, checks hop and pair limits, writes the ticket and message, and spawns `run_ticket`.
3. `run_ticket` reads Claude's wake kind (`cli`), builds the prompt, and calls `wakers::run_wake`. That asks `providers::get("claude").wake(…)` for `claude -p … --permission-mode plan` and runs it in the room's folder.
4. Claude's stdout becomes the answer: `core.finish` stores it, notifies waiters, runs any `then` handoff, and exports Markdown.
5. Cursor's `mux_ask_peer` returns it. If it took too long, Cursor gets a pending ticket and later uses `mux_get_reply` or `mux_updates`.
