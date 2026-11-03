# Contributing

Thanks for helping. Issues and pull requests are welcome, and **new providers** are the most useful contribution: every agent you add can talk to all the others.

Full guides live in [`docs/`](docs) and at [agentmux.dev/docs](https://agentmux.dev/docs):

- [Development setup](docs/development.md): build, run against a scratch vault, tests
- [Architecture](docs/architecture.md): processes, modules, the life of a question
- [Adding a provider](docs/adding-a-provider.md): one Rust file behind the `Provider` trait
- [Custom agents](docs/custom-agents.md): a TOML manifest, no Rust

**Quick start:** `bun install`, then `bun run dev` (Tauri dev app), plus `cargo test -p agentmux-core`.

- **Layout:**
  - `crates/agentmux-core`: daemon, MCP tools, vault, providers, gateway and tunnels;
  - `crates/agentmux-cli`: the `agentmux` binary;
  - `src-tauri`: the desktop shell;
  - `src`: the React UI;
  - `docs`: documentation.
- **Before a PR:** `cargo test -p agentmux-core` and `bunx tsc --noEmit` must pass. Add a test for new core behaviour. Keep provider-specific code in its provider file.
- **Security issues:** don't open public issues. See [SECURITY.md](SECURITY.md).

By contributing you agree that your work is licensed under Apache-2.0.
