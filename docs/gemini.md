# Gemini CLI

| | |
|---|---|
| Id | `gemini` |
| File | `providers/gemini.rs` |
| MCP config | `~/.gemini/settings.json` → `mcpServers.agentmux` (backup `settings.json.agentmux.bak`) |
| Wake | `gemini -p` |
| Special | **Chat hook** in `~/.gemini/settings.json` (like Claude Code's) |

## Connect

```sh
agentmux connect gemini
```

AgentMux adds `{"command": "~/.agentmux/bin/agentmux", "args": ["mcp", "--agent", "gemini"]}` under `mcpServers.agentmux`, plus the chat hook (below). It keeps everything else in the file (your model, other MCP servers, UI settings), and the first edit makes a backup. New Gemini CLI sessions see it. Install the CLI with `npm install -g @google/gemini-cli`.

## Wake

```sh
gemini -p "<prompt>" --approval-mode plan --output-format text        # read-only (default)
gemini -p "<prompt>" --approval-mode auto_edit --output-format text   # room allows edits
```

`plan` is Gemini's read-only mode. With edits allowed, `auto_edit` approves file edits but still not arbitrary shell commands.

## The chat hook

Gemini CLI runs hooks on each message (`BeforeAgent`) and when a chat starts or resumes (`SessionStart`), so it gets the same treatment as [Claude Code](./claude-code.md#the-context-hook). AgentMux adds, in `~/.gemini/settings.json`:

```json
{
  "hooks": {
    "BeforeAgent":  [{ "hooks": [{ "type": "command", "command": "~/.agentmux/bin/agentmux hook gemini", "name": "agentmux" }] }],
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "~/.agentmux/bin/agentmux hook gemini", "name": "agentmux" }] }]
  }
}
```

- **Late replies land in the same chat.** When another agent answers after Gemini stopped waiting, the answer shows up in the same Gemini chat on your next message. The same goes for questions addressed to Gemini and thread notices.
- **Each chat keeps its thread.** Questions a chat asks without a topic continue its AgentMux thread. When a chat starts or resumes, Gemini is told which thread it's on and where it stands.
- **Output format.** Gemini CLI wants JSON from hooks, so the command prints `{"hookSpecificOutput": {"additionalContext": "…"}}`, or `{}` when nothing is new. It always exits 0, and it stays silent inside AgentMux's own headless runs.
- **Switching it off.** Use Connect → Gemini CLI, or `agentmux hook gemini --off` / `--on`. Disconnecting removes only AgentMux's entries.
