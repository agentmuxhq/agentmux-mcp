# Development setup

## Requirements

- macOS (the only supported platform for now)
- Rust stable (`rustup`)
- [bun](https://bun.sh)
- Xcode command line tools (`xcode-select --install`)

## Run it

```sh
git clone https://github.com/agentmuxhq/agentmux-mcp
cd agentmux-mcp
bun install
bun run dev                     # Tauri dev app (also builds the agentmux CLI)
```

The dev app uses your real vault in `~/.agentmux`. To experiment safely, point everything at a scratch vault:

```sh
export AGENTMUX_HOME=$(mktemp -d)
cargo run -p agentmux-cli -- daemon
cargo run -p agentmux-cli -- call mux_contacts --as claude
```

## Repository layout

| Path | What |
|---|---|
| `crates/agentmux-core` | Everything that matters: daemon, MCP tools, vault, providers, gateway, tunnels, sync |
| `crates/agentmux-core/src/providers/` | One file per provider ([Architecture](./architecture.md)) |
| `crates/agentmux-cli` | The `agentmux` binary: shim, daemon, commands, the Claude Code hook |
| `src-tauri` | The desktop shell: Tauri commands, menu bar, updater |
| `src` | The React UI |
| `docs` | These docs (also published at agentmux.dev/docs) |
| `npm`, `scripts` | npm packages, release and Homebrew scripts |

## Tests

```sh
cargo test -p agentmux-core     # core: tickets, sessions, gateway, webhooks, providers, hook, sync
cargo test -p agentmux-cli      # the agentmux binary: shim reconnect across daemon restarts, login service
bunx tsc --noEmit               # UI types
```

The core tests never touch your vault, Keychain or real agents. They use temp vaults, fake CLIs (small shell scripts on a temp `PATH`), and local fake webhook servers. New core behaviour needs a test. Look at `tests/providers.rs`, `tests/manifest.rs` and `tests/grok_webhook.rs` for the patterns.

## Pull requests

- `cargo test -p agentmux-core -p agentmux-cli` and `bunx tsc --noEmit` must pass (CI runs both).
- Keep provider-specific code inside its provider file.
- Never log or print secrets. Tokens and keys go to the Keychain through `secrets.rs`.
- Security issues: don't open a public issue; see the [security model](./security.md).
