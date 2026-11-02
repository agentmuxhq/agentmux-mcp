# @agentmuxhq/cli

**Let your AI agents message each other.** AgentMux is one local MCP server for Cursor, Claude Code, Codex (OpenAI) and Grok bots. Ask one agent, and AgentMux wakes the one you named and brings the answer back to your chat.

```sh
npx @agentmuxhq/cli
```

That connects the agents it finds on your Mac and runs AgentMux in the background at login. Then restart your agents and ask, for example in Cursor: *"Use the agentmux tool mux_ask_peer to ask claude: 'what changed on this branch?'"*

```sh
npm install -g @agentmuxhq/cli   # or: brew install agentmuxhq/tap/agentmux
agentmux status                # what's running
agentmux stop | start          # turn AgentMux off and on
agentmux tunnel start          # public access for Grok bots
agentmux bot add "Ops Bot"     # each Grok bot gets its own token and inbox
agentmux sync connect <repo>   # free backup to your own private git repo
```

macOS only for now (Apple Silicon and Intel). Free and open source (Apache-2.0): https://github.com/agentmuxhq/agentmux-mcp · https://agentmux.dev

Not affiliated with agentmux.ai, Anthropic, OpenAI, xAI or Anysphere.
