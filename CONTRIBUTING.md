# Contributing

Thanks for helping. Issues and pull requests are welcome.

- **Setup:** `bun install`, then `bun run dev` (Tauri dev app) and `cargo test -p agentmux-core`.
- **Layout:**
  - `crates/agentmux-core`: daemon, MCP tools, vault, wakers, gateway and tunnels;
  - `crates/agentmux-cli`: the `agentmux` binary;
  - `src-tauri`: the desktop shell;
  - `src`: the React UI.
- **Before a PR:** `cargo test -p agentmux-core` and `bunx tsc --noEmit` must pass. Add a test for new core behaviour.
- **Security issues:** don't open public issues. See [SECURITY.md](SECURITY.md).

By contributing you agree that your work is licensed under Apache-2.0.
