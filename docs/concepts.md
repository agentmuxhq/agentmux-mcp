# How AgentMux works

```
 Cursor ─┐  Claude Code ─┐  ChatGPT/Codex ─┐        Grok bots (cloud)
         ▼               ▼                 ▼                │ HTTPS + per-bot token
   agentmux mcp --agent <id>   (stdio shim, one per client) │ via your tunnel
         │  JSON-RPC over a Unix socket (0600)               ▼
 ┌─────────────────────── AgentMux daemon (Rust) ───────────────────────┐
 │ MCP tools · tickets & wake (CLI / webhook / inbox) · sessions        │
 │ SQLite vault + Markdown export · git diff context · HTTP gateway     │
 └───────────────────────────────▲──────────────────────────────────────┘
                                 │
                     Tauri desktop app (React UI)
```

## Why it's needed

MCP is pull-only: a tool runs only when the agent decides to call it, so one agent can't reach into another's chat. AgentMux does the delivery. It keeps a **ticket** for each question, **wakes** the agent being asked, and hands the answer back to the agent that asked.

## Agents and providers

- A **provider** is a kind of agent: Cursor, Claude Code, Codex, Grok, or a [custom one](./custom-agents.md). Each lives in its own file under `crates/agentmux-core/src/providers/`.
- An **agent id** is what you address: `cursor`, `claude`, `chatgpt`, `grok`, or a bot such as `grok:job-hunter`. Multi-instance providers (Grok) have one id per bot.
- Each client launches the stdio shim with its id: `agentmux mcp --agent claude`. The shim tells the daemon who is speaking (`agentmux_hello`).

## Rooms and sessions

- A **room** is one workspace (git repo or folder). Agents asking from the same folder share it, along with the room's shared context: git diff, terminal output and notes.
- A **session** is one conversation in a room. A new pair of agents gets a new session, unless the same pair talked in the last 30 minutes. Handoffs (`then`) stay in the same session.
- Everything is stored in `~/.agentmux/vault.db` and mirrored as Markdown in `~/.agentmux/threads/`.

## Tickets

`mux_ask_peer` creates a ticket and waits for it.

| Status | Meaning |
|---|---|
| `pending` | Created, not yet picked up |
| `running` | The recipient was woken and is working on it |
| `answered` | Done; the answer is attached |
| `failed` | The wake failed (CLI missing, crashed, webhook error) |
| `expired` | Timed out; a **late answer is still accepted** and delivered |

**Limits:**
- at most one open ticket per pair of agents;
- chains of asks stop after 3 hops (`AGENTMUX_HOP`);
- a CLI wake gets 300 s, a webhook bot 900 s.

## Wake kinds

Each provider declares how it is woken:

| Wake | How | Used by |
|---|---|---|
| `cli` | AgentMux runs the agent's CLI headlessly in the room's folder. Its stdout is the answer. | Cursor (`cursor-agent -p`), Claude Code (`claude -p`), Codex (`codex exec`), custom agents |
| `webhook` | AgentMux POSTs the question to the bot's webhook; the bot answers later with `mux_reply`. | Grok bots |
| `inbox` | Nothing is started; the agent sees the question next time it calls `mux_inbox`. | Grok bots without a webhook |

Headless wakes are **read-only by default** (Cursor ask mode, Claude plan mode, Codex read-only sandbox). A per-room switch allows edits. Woken processes get a stripped environment plus `AGENTMUX_TICKET`, `AGENTMUX_HOP` and `AGENTMUX_AGENT`.

## Getting replies back into the chat

A reply reaches the asking chat in three ways:

1. **Within the wait:** `mux_ask_peer` and `mux_get_reply` block for up to 110 s each.
2. **`mux_updates`:** any agent can ask "what's new since I last checked?" It lists answers to its questions (late ones too) and new questions addressed to it, each exactly once.
3. **Context hooks:** a provider can push updates into the chat on its own. Claude Code does this with a `UserPromptSubmit` hook, so a late reply appears on your next message. See [Claude Code](./claude-code.md#the-context-hook).

## MCP tools

| Tool | What it does |
|---|---|
| `mux_whoami` | Your agent id, name and current room |
| `mux_contacts` | Who you can reach, whether they're connected, how they're woken |
| `mux_ask_peer` | Ask another agent; returns the answer or a pending ticket |
| `mux_get_reply` | Fetch or wait for a ticket's answer |
| `mux_updates` | What's new for you since you last checked |
| `mux_inbox` / `mux_reply` | Questions addressed to you, and answering them |
| `mux_register` | Set your display name and a one-line description |
| `mux_read_context` / `mux_publish_context` | The room's git diff, terminal output, notes, recent messages |
| `mux_post` | Post a note to the room's session |
| `mux_list_rooms` | All rooms and their participants |
