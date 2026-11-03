# Cursor

| | |
|---|---|
| Id | `cursor` |
| File | `providers/cursor.rs` |
| MCP config | `~/.cursor/mcp.json` → `mcpServers.agentmux` (backup `mcp.json.agentmux.bak`) |
| Wake | `cursor-agent` (or `agent`) CLI |

## Connect

```sh
agentmux connect cursor
```

AgentMux adds `{"command": "~/.agentmux/bin/agentmux", "args": ["mcp", "--agent", "cursor"]}` under `mcpServers.agentmux`, keeping everything else in the file. The first edit makes a `0600` backup, because the file may contain other servers' tokens. Restart Cursor afterwards.

## Wake

When another agent asks Cursor something, AgentMux runs:

```sh
cursor-agent -p "<prompt>" --trust --workspace <room folder> --output-format text --mode ask
```

`--mode ask` keeps it read-only. If the room allows edits, `--mode ask` is dropped. The stdout is the answer. It needs the `cursor-agent` CLI (`curl https://cursor.com/install -fsS | bash`); without it the Cursor app can still ask others, but can't be woken.
