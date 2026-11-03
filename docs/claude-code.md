# Claude Code

| | |
|---|---|
| Id | `claude` |
| File | `providers/claude_code.rs`, `crates/agentmux-cli/src/hook.rs` |
| MCP config | `claude mcp add --scope user agentmux -- ~/.agentmux/bin/agentmux mcp --agent claude` |
| Wake | `claude -p` |
| Special | **Context hook** in `~/.claude/settings.json` |

## Connect

```sh
agentmux connect claude          # MCP server + context hook
agentmux connect claude --no-hook   # MCP server only
```

## Wake

```sh
claude -p "<prompt>" --permission-mode plan          # read-only (default)
claude -p "<prompt>" --permission-mode acceptEdits   # room allows edits
```

## The context hook

MCP can't push into a chat: Claude only sees an answer if it calls a tool. When Claude asks Grok something and Grok takes 10 minutes, the reply would normally sit in AgentMux until someone looks.

The hook fixes that for Claude Code. AgentMux adds two entries to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "~/.agentmux/bin/agentmux hook claude-code" }] }],
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "~/.agentmux/bin/agentmux hook claude-code" }] }]
  }
}
```

Each time you send a message (and when a session starts), Claude Code runs `agentmux hook claude-code`, which:

1. reads Claude's unread updates from the vault (the same data as `mux_updates`): answers to Claude's questions, including late ones, and questions other agents asked Claude;
2. prints a short summary (at most about 1,500 characters), which Claude Code adds to the conversation as context;
3. advances Claude's read cursor, so each update is shown once.

So the reply appears **in the same chat you asked from**, on your next message:

> AgentMux: since your last message, other agents got back to you.
> \- Job Hunter answered your question "any new listings?" (ticket T-…): 14 new listings, 2 auth errors

**Guarantees:**
- **Never in the way:** it always exits 0, prints nothing when there's nothing new, takes about 20 ms, and reads the vault directly, so it works even when the daemon is off.
- **Skips AgentMux's own runs:** headless `claude -p` wakes carry `AGENTMUX_TICKET`, and the hook stays silent there, so a background wake never consumes your updates.
- **Leaves your config alone:** your other hooks are untouched, the first edit keeps a `settings.json.agentmux.bak`, and `--off`/`disconnect` remove only AgentMux's entries.

Turn it on or off in the app (**Connect → Claude Code → Tell Claude Code what's new…**) or with:

```sh
agentmux hook claude-code --off
agentmux hook claude-code --on
```
