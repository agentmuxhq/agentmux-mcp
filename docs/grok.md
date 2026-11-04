# Grok bots

| | |
|---|---|
| Ids | `grok` (the first bot) and `grok:<slug>` for every bot you add |
| File | `providers/grok.rs`, plus `webhook.rs` and `gateway.rs` |
| Wake | Webhook (a Grok Routine), or the inbox |
| Special | **Multi-instance**, **remote access** over a tunnel, **per-bot token**, **can start conversations** |

Grok bots run in xAI's cloud, not on your Mac. That makes them the provider with the most special handling.

## One contact per bot

You can have many Grok bots (a job hunter, an ops bot, a researcher). Each is its own contact with its own:

- **id**: `grok:<slug>`, created with `agentmux bot add "Job Hunter"` → `grok:job-hunter`;
- **bearer token**, stored in the macOS Keychain (`grok-mcp-token:<slug>`);
- **webhook** URL (`webhook.<id>.url`) and key (Keychain `grok-webhook-key:<slug>`);
- **inbox**, so answers never mix between bots.

A bot can name itself: if `mux_whoami` says `registered: false`, its routine calls `mux_register` with its name and a one-line description.

## Reaching your Mac: the tunnel

A cloud bot can't reach `127.0.0.1`, so AgentMux exposes its HTTP gateway (`127.0.0.1:4317`, `POST /mcp`) through a tunnel you control:

| Mode | URL | Use |
|---|---|---|
| Cloudflare quick tunnel (default) | New random `*.trycloudflare.com` each start | Trying it out |
| Cloudflare named tunnel | Your own subdomain, e.g. `mux.example.com`, permanent | Daily use; set up from **Settings → Public access** (log in, pick a zone, AgentMux creates and routes the tunnel and verifies `/healthz`) |
| ngrok | A reserved static domain | If you prefer ngrok (authtoken kept in the Keychain) |

```sh
agentmux tunnel start
agentmux bot token grok:job-hunter   # prints the MCP URL and the bot's token
```

Paste the URL (`https://<host>/mcp`) and token into Grok as a custom MCP connector.

**If a permanent tunnel's settings ever disappear,** AgentMux won't quietly start a temporary URL in its place, because that would leave Grok's connector pointing at nothing. It keeps the last working config, shows **Your permanent tunnel was cleared** in Public access and the menu bar, and **Restore** (or `agentmux tunnel restore`) puts it back. Every change to tunnel settings is logged with the process that made it in `~/.agentmux/logs/settings.log`. Tunnels are off by default, stop when you quit from the menu bar, and can autostart if you choose.

## What a bot's token allows

Remote callers never get the full tool set. A token is checked in constant time, bound to one bot, and the request's `Host` must match the active tunnel. It unlocks:

| Always | `mux_whoami`, `mux_contacts`, `mux_register`, `mux_inbox`, `mux_reply`, `mux_updates`, `mux_sessions` (its own room only) |
|---|---|
| **With "start conversations"** (on by default) | `mux_ask_peer`, `mux_get_reply` |
| Never | `mux_read_context`, `mux_publish_context`, `mux_post`, `mux_list_rooms` |

**Start conversations** is the bot-specific permission. When it's on:

- the bot can ask Cursor, Claude Code, ChatGPT or other bots, up to **30 new questions an hour**;
- room and session arguments are stripped, so its questions never carry your repo's git diff or notes;
- agents it wakes stay **read-only**, and the 3-hop and one-open-ticket-per-pair limits still apply.

Switch it off per bot in **Connect → Grok Bot → Let this bot start conversations**, or:

```sh
agentmux bot ask grok:job-hunter off
```

A bot can only `mux_reply` to tickets addressed to itself.

## Waking a bot: webhooks

Create a Routine in Grok with a **Webhook** trigger and paste the instructions from the Connect screen (they tell the bot to answer through `mux_reply`). Then save the routine's URL and key:

```sh
agentmux bot webhook grok:job-hunter https://…/automations/webhook/<routine-id>
```

When someone asks the bot something, AgentMux POSTs the ticket id, question, room name, shared notes and up to 6,000 characters of git diff to the webhook. The routine runs, then calls `mux_reply`. The ticket stays `running` for up to 900 s and then expires, but **a late answer is still delivered**.

Webhooks must be https (loopback http is allowed for tests). Credentials are never put in the URL, and redirects are disabled so the key is never forwarded.

Without a webhook, a bot is inbox-only: questions wait until it calls `mux_inbox`.
