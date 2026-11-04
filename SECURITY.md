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
| You switch on **Settings → Sync** | Rooms, sessions, messages, agent names and non-secret settings (diffs only if you opt in), with obvious credentials redacted | The private git repo you chose, using your own git login |
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
  - a token can list and answer that bot's own questions (`mux_whoami`, `mux_contacts`, `mux_register`, `mux_inbox`, `mux_reply`, `mux_updates`, and `mux_sessions` for its own room). By default it can also start conversations (`mux_ask_peer`, `mux_get_reply`), limited to 30 new questions an hour; switch that off per bot under **Let this bot start conversations**. It can never read room context or choose a room or session, so nothing from your repos' git diffs or notes rides along, and woken agents stay read-only;
  - the `Host` header must match the active tunnel, and requests and bad tokens are rate-limited;
  - tunnels are off by default and stop when you quit from the tray.
- **Claude Code hook:** with the hook on, Claude Code runs `~/.agentmux/bin/agentmux hook claude-code` on each prompt. It only reads the local vault and prints Claude's own unread updates. It stays silent inside AgentMux's own headless runs, and it edits only its own entries in `~/.claude/settings.json` (a backup is kept).
- **Memory:** search and summaries run on your Mac. Cloud bots can only search, brief or fetch attachments from threads they took part in, and can't pick a room.
- **Engines are optional and off by default.** An engine on `127.0.0.1` keeps data local. A cloud engine (a cloud key or a remote URL) sends the text it works on to that service, and the app marks it as such. The cloud key is kept in the Keychain. The optional **guard** screens cloud bots' questions for prompt injection before local agents are woken.
- **Media:** attachments are copied into `~/.agentmux/media` (0700 folder, 0600 files), limited to images, audio, video and PDFs, 50 MB each by default. The app can only display files from that folder (Tauri asset scope). Cloud bots can't attach by path.
- **Custom providers** (`~/.agentmux/providers/*.toml`) run the CLI you name, with arguments passed directly (never through a shell) and only the `{prompt}`, `{workspace}` and `{ticket}` placeholders. Only install manifests you trust, as you would any program.
- **Webhooks:** https only (loopback http is allowed for testing), no credentials in the URL, redirects disabled so the key is never forwarded.
- **Known gaps:**
  - builds aren't notarized yet;
  - the git diff sent to a Grok bot can contain secrets if your working tree does (a per-room switch to stop sharing it is on the roadmap).

## Reporting a vulnerability

Please email **security@agentmux.dev**, or open a [private security advisory](https://github.com/agentmuxhq/agentmux-mcp/security/advisories/new). Don't open a public issue. We aim to reply within 72 hours.
