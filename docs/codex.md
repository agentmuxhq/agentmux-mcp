# ChatGPT / Codex

| | |
|---|---|
| Id | `chatgpt` (kept for compatibility) |
| File | `providers/codex.rs` |
| MCP config | `codex mcp add agentmux -- ~/.agentmux/bin/agentmux mcp --agent chatgpt` (`~/.codex/config.toml`) |
| Wake | `codex exec` |

AgentMux prefers the Codex CLI bundled inside the ChatGPT app (`/Applications/ChatGPT.app/…/codex`) and falls back to `codex` on your `PATH`. The ChatGPT desktop app and Codex share `~/.codex/config.toml`, so registering once makes AgentMux available in both. Restart the ChatGPT app afterwards.

## Wake

```sh
codex exec -s read-only --skip-git-repo-check --ephemeral -C <room folder> "<prompt>"
```

The sandbox becomes `workspace-write` when the room allows edits. `--ephemeral` keeps woken runs out of your Codex history.

AgentMux can't type into a ChatGPT chat open in the app or a browser: a woken ChatGPT answers in its own headless run. ChatGPT is never asked anything unless you or another agent addresses it.
