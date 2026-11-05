# Providers

A **provider** is a kind of agent AgentMux can talk to. Each one is a single Rust file in [`crates/agentmux-core/src/providers/`](https://github.com/agentmuxhq/agentmux-mcp/tree/main/crates/agentmux-core/src/providers) that implements the `Provider` trait. The rest of AgentMux (daemon, MCP tools, gateway, app UI) only looks at what the provider declares. It never matches on provider names.

| Provider | Id | Wake | Special |
|---|---|---|---|
| [Cursor](./cursor.md) | `cursor` | `cursor-agent -p` | Edits `~/.cursor/mcp.json` |
| [Claude Code](./claude-code.md) | `claude` | `claude -p` | **Context hook**: late replies show up in the same chat |
| [ChatGPT / Codex](./codex.md) | `chatgpt` | `codex exec` | Uses the Codex CLI bundled with the ChatGPT app |
| [Gemini CLI](./gemini.md) | `gemini` | `gemini -p` | **Chat hook** like Claude Code's; read-only wakes use plan mode |
| [Grok bots](./grok.md) | `grok`, `grok:<slug>` | Webhook or inbox | **Multi-instance**, **remote access** over a tunnel with a per-bot token |
| [Custom](./custom-agents.md) | anything | Your CLI | A TOML manifest, no Rust needed |

## Capabilities

What a provider can do is declared in `Capabilities`, and everything else follows from it:

| Capability | Meaning | Effect |
|---|---|---|
| `wake` | `cli`, `webhook` or `inbox` | How a question to it is delivered (see [wake kinds](./concepts.md#wake-kinds)) |
| `multi_instance` | Many named instances (`grok:<slug>`) | The app shows a bot picker; ids with a `:` suffix resolve to this provider |
| `remote_access` | Reachable over the HTTP gateway | Gets a bearer token per instance and a restricted tool set; set up by hand in the provider's own app |
| `context_hook` | Can show AgentMux updates in its own chat | The app shows a hook switch; `set_option("hook", …)` turns it on or off |

Provider-specific switches (`set_option` / `options`) appear in the app's Connect screen and in the CLI. Claude Code's `hook` is the first.

Want to add one? See [Adding a provider](./adding-a-provider.md), or for a simple CLI agent, [Custom agents](./custom-agents.md).
