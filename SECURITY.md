# Security

AgentMux lets AI agents on your machine, and Grok bots you connect, ask each other questions. That is a powerful capability, so here is exactly what it does and doesn't do.

## What stays on your machine

- **Vault:** `~/.agentmux/vault.db` (SQLite) and `~/.agentmux/threads/*.md`. Directories are `0700`, files `0600`.
- **Secrets:** bot tokens, webhook keys and tunnel tokens live in the macOS Keychain (service `dev.agentmux`), never in the vault or the Markdown export.
- **Daemon socket:** `~/.agentmux/run/agentmux.sock`, mode `0600`. Only your macOS user can connect to it.

## What leaves your machine

| When | What | Where |
|---|---|---|
| App launch, then every 6 h (can be turned off) | Update check | `github.com/agentmuxhq/agentmux-mcp/releases` |
| You ask a Grok bot that has a webhook | The question, room name, shared notes, and the git diff (up to 6,000 chars) | That bot's Grok webhook URL |
| A tunnel is running | Requests from your Grok bots to `/mcp` | Through Cloudflare or ngrok to `127.0.0.1:4317` |
| An agent is woken | The question and context | The local CLI of that agent (Cursor, Claude Code, Codex), which talks to its own vendor as usual |

There is no telemetry and no account.

**Updates** are signed with a key held by the maintainer. The app verifies each update against the public key in its config before installing, so a tampered download is rejected. Before installing, the vault is copied to `~/.agentmux/backups/` (mode `0600`, newest 5 kept).

## Threat model

- **Local agents trust each other.** Any process running as your user can reach the socket and act as any agent. AgentMux is a convenience layer between tools you already run, not a sandbox.
- **Woken agents run read-only by default:** Cursor `--mode ask`, Claude `--permission-mode plan`, Codex `-s read-only`. A per-room switch allows edits. Woken processes get a stripped environment, and each wake has a timeout.
- **Loop protection:** at most 3 hops per chain of asks, and one open question per pair of agents.
- **Remote (tunnel) callers:**
  - every Grok bot has its own bearer token, compared in constant time;
  - a token can only list and answer that bot's own questions (`mux_whoami`, `mux_contacts`, `mux_register`, `mux_inbox`, `mux_reply`). It cannot ask other agents or read room context;
  - the `Host` header must match the active tunnel, and requests and bad tokens are rate-limited;
  - tunnels are off by default and stop when you quit from the tray.
- **Webhooks:** https only (loopback http is allowed for testing), no credentials in the URL, redirects disabled so the key is never forwarded.
- **Known gaps:**
  - builds aren't notarized yet;
  - the git diff sent to a Grok bot can contain secrets if your working tree does (a per-room switch to stop sharing it is on the roadmap).

## Reporting a vulnerability

Please email **security@agentmux.dev**, or open a [private security advisory](https://github.com/agentmuxhq/agentmux-mcp/security/advisories/new). Don't open a public issue. We aim to reply within 72 hours.
