# Getting started

AgentMux lets the AI agents on your Mac (Cursor, Claude Code, ChatGPT/Codex) and your Grok bots ask each other questions. It runs locally: one small Rust daemon, a local vault, and an MCP server every agent connects to.

## Install

Pick one. They all install the same `agentmux` command and share the same data in `~/.agentmux`.

**npx (no install, recommended):**

```sh
npx @agentmuxhq/cli
```

**Homebrew:**

```sh
brew install agentmuxhq/tap/agentmux
agentmux init
```

**Desktop app** (menu bar, session view, settings): download the `.dmg` from [Releases](https://github.com/agentmuxhq/agentmux-mcp/releases/latest), `aarch64` for Apple Silicon, `x64` for Intel. It isn't notarized yet; on first open use **System Settings → Privacy & Security → Open Anyway**.

`init` does three things:

1. links a stable binary at `~/.agentmux/bin/agentmux`;
2. registers AgentMux as an MCP server in each agent it finds (Cursor, Claude Code, Codex), and for Claude Code also adds the [context hook](./claude-code.md#the-context-hook);
3. installs a login service so the daemon runs in the background.

Restart your agents afterwards so they load the new MCP server.

## Your first question

In any connected agent, type:

```
Use the agentmux tool mux_ask_peer to ask claude: "what changed on this branch?"
```

AgentMux wakes Claude Code headlessly in the same workspace (read-only), and the answer comes back to the chat you asked from.

If the other agent takes longer than the wait (up to 110 s per call), you get a pending ticket. The asking agent keeps calling `mux_get_reply` until it's answered. If it stops, the answer still arrives: `mux_updates` lists it on the next turn, and in Claude Code the hook shows it automatically on your next message.

## Check it's working

```sh
agentmux status
```

It shows the daemon, tunnel, each agent and when it was last seen. In the app, **Connect a provider** has a **Send test ping** button per agent.

## Next

- [How AgentMux works](./concepts.md): rooms, sessions, tickets and wakes.
- [Grok bots](./grok.md): connecting cloud agents through a tunnel.
- [Adding a provider](./adding-a-provider.md): bring your own agent.
