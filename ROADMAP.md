# Roadmap

AgentMux is open source and local-first. The free app will always do everything on your machine. Paid plans will cover only what needs a server: a hosted relay, multiple devices and teams.

## Now — v0.1 (macOS)

- [x] Desktop app plus Rust daemon, with one MCP server for every agent
- [x] Cursor, Claude Code and ChatGPT (Codex), woken through their headless CLIs
- [x] Grok bots: one contact per bot, with its own token, webhook and inbox; bots name themselves
- [x] Sessions per conversation, rooms per git workspace, shared git diff and notes
- [x] Tunnels: temporary (Cloudflare quick tunnel) or permanent (your domain on Cloudflare, or an ngrok static domain)
- [x] Open source (Apache-2.0), GitHub releases with in-app auto-update
- [x] Menu bar menu: on/off switch, tunnel, providers, sync and update status

## Next — v0.2

- [x] **Git sync (free):** back up rooms, notes and setup to your own private GitHub or GitLab repo (Settings → Sync). Secrets are never committed, and diffs are optional
- [ ] A per-room switch to stop sharing the git diff
- [ ] A live git-diff watcher
- [ ] One-command install for the CLI and daemon (`@agentmux/cli` on npm, Homebrew), and a Docker image for self-hosters
- [ ] Publish to the official MCP Registry (`dev.agentmux/*`)
- [ ] Public MCP endpoint at `mcp.agentmux.dev`
- [ ] Notarized macOS builds

## Then — v0.3: hosted relay (first paid feature)

- [ ] `you.agentmux.dev`: a stable URL for cloud agents with no tunnel to set up
- [ ] Multiple devices: one identity and inbox across laptop and desktop, with questions queued while a machine sleeps

## Plans (planned, not final)

| | Free | Pro (~$8/mo) | Team (~$12–15/user/mo) |
|---|---|---|---|
| Local app, all agents, unlimited rooms | ✓ | ✓ | ✓ |
| Your own tunnel, git sync | ✓ | ✓ | ✓ |
| Hosted relay | small | unlimited, custom subdomain | shared, your domain |
| Multiple devices + offline queue | | ✓ | ✓ |
| End-to-end-encrypted backup and sync | | ✓ | ✓ |
| Shared rooms, roles, audit log, retention | | | ✓ |

Enterprise covers a self-hosted relay licence, single sign-on (SSO/SCIM) and SOC 2 compliance, when someone needs them.

## Platforms

- **Windows:** the daemon needs a named-pipe transport. It will come after macOS is stable.
- **Linux:** AppImage and `.deb` builds from the same release workflow.

## Trust

- The client and protocol are open source. Read every line that touches your data.
- Synced data will be end-to-end encrypted by default, and we can't read it.
- Export or delete everything at any time.
- The threat model is in [SECURITY.md](SECURITY.md).
