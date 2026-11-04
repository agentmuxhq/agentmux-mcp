# Gemini CLI

| | |
|---|---|
| Id | `gemini` |
| File | `providers/gemini.rs` |
| MCP config | `~/.gemini/settings.json` → `mcpServers.agentmux` (backup `settings.json.agentmux.bak`) |
| Wake | `gemini -p` |

## Connect

```sh
agentmux connect gemini
```

AgentMux adds `{"command": "~/.agentmux/bin/agentmux", "args": ["mcp", "--agent", "gemini"]}` under `mcpServers.agentmux`. It keeps everything else in the file (your model, other MCP servers, UI settings), and the first edit makes a backup. New Gemini CLI sessions see it. Install the CLI with `npm install -g @google/gemini-cli`.

## Wake

```sh
gemini -p "<prompt>" --approval-mode plan --output-format text        # read-only (default)
gemini -p "<prompt>" --approval-mode auto_edit --output-format text   # room allows edits
```

`plan` is Gemini's read-only mode. With edits allowed, `auto_edit` approves file edits but still not arbitrary shell commands.
