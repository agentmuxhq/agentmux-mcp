# CLI reference

Every command is `agentmux <command>`. Run `agentmux --help` or `agentmux <command> --help` for the full flags.

| Command | What it does |
|---|---|
| `init [--no-service] [--no-connect]` | Set up this Mac: link the binary, connect agents, install the login service |
| `start` / `stop` | Turn AgentMux on or off (stop also stops the tunnel; nothing restarts it until `start`) |
| `status` | Daemon, tunnel, agents and sync at a glance |
| `connect <agent\|all> [--no-hook]` | Register AgentMux as an MCP server in an agent. `--no-hook` skips Claude Code's context hook |
| `disconnect <agent\|all>` | Remove it again (and Claude Code's hook) |
| `hook claude-code [--on\|--off]` | With no flag, this is what Claude Code runs on each prompt. `--on` / `--off` add or remove the hook |
| `provider list` | All providers, built-in and custom |
| `provider add <file.toml>` / `provider remove <id>` | Install or remove a [custom agent](./custom-agents.md) |
| `tunnel status\|start\|stop\|test\|restore\|keep-temporary` | Public access for Grok bots. `restore` brings back a permanent tunnel whose settings were cleared; `keep-temporary` gives it up |
| `bot list\|add <name>\|remove <id>\|token <id> [--rotate]\|webhook <id> <url>\|ask <id> on\|off` | Manage Grok bots |
| `sync status\|connect <remote>\|now\|off [--diffs] [--manual]` | Back up to your own private git repo |
| `call <tool> '<json>' [--as <agent>]` | Call an MCP tool through the daemon (testing) |
| `run -- <command…>` | Run a command and publish its output as the room's terminal context |
| `daemon` | Run the daemon in the foreground (what the login service runs) |
| `mcp --agent <id>` | The stdio shim agents launch |
| `uninstall [--keep-configs]` | Remove the login service and, unless kept, AgentMux from agents' configs |

## Environment variables

| Variable | Effect |
|---|---|
| `AGENTMUX_HOME` | Vault location (default `~/.agentmux`). Tests use a temp dir |
| `AGENTMUX_PORT` | HTTP gateway port (default `4317`) |
| `AGENTMUX_WAKE_TIMEOUT_SECS` | Override wake timeouts |
| `AGENTMUX_CLAUDE_SETTINGS` | Claude Code settings file the hook installer edits (tests) |
