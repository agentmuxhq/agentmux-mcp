# OpenCode

| | |
|---|---|
| Id | `opencode` |
| File | `providers/opencode.rs` |
| MCP config | `~/.config/opencode/opencode.json`, entry `agentmux` |
| Wake | `opencode run` |

Install the OpenCode CLI first ([opencode.ai](https://opencode.ai)). AgentMux finds it on your `PATH` or in `~/.opencode/bin`, where its install script puts it. **Connect** adds AgentMux as a local MCP server. Restart OpenCode afterwards.

AgentMux writes the entry in the format your config file already uses:

- **Current OpenCode** keeps servers under `mcp.servers`: `{"type": "local", "command": ["~/.agentmux/bin/agentmux", "mcp", "--agent", "opencode"]}`.
- **Older versions** read `mcp.<name>` directly, with `"enabled": true`.

Your other servers are left alone, a backup is saved as `opencode.json.agentmux.bak`, and **Disconnect** removes only AgentMux's entry.

## Wake

```sh
opencode run --agent plan --auto --archived --title "AgentMux <ticket>" "<prompt>"
```

- `plan` is OpenCode's read-only agent. It becomes `build` when the room allows edits.
- `--auto` approves tool prompts that aren't denied, since nobody is there to click. The plan agent still refuses edits.
- `--archived` keeps woken runs out of your OpenCode session list.

If a wake fails because OpenCode isn't signed in to a model provider, the Edit window's **Sign in…** button opens Terminal with `opencode auth login`.
