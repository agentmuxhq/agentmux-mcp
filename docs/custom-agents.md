# Custom agents (no Rust)

If your agent has a CLI that takes a prompt and prints an answer, you can add it with a TOML file and no code. Drop it in `~/.agentmux/providers/`, or install it with:

```sh
agentmux provider add aider.toml
agentmux connect aider
```

Restart AgentMux (or the app) to load a new manifest.

## Example

```toml
id = "aider"
name = "Aider"
color = "#14B8A6"

# How to tell it's installed (looked up on PATH plus the usual install dirs).
detect = { binary = "aider" }

# How to register AgentMux as an MCP server in it. Either merge into a JSON config...
mcp = { kind = "json-merge", path = "~/.aider/mcp.json", key = "mcpServers.agentmux" }
# ...or run its own commands ({link} is the AgentMux binary):
# mcp = { kind = "command", add = ["mcp", "add", "agentmux", "--", "{link}", "mcp", "--agent", "aider"], remove = ["mcp", "remove", "agentmux"] }

# How to wake it headlessly. stdout is the answer.
wake = { binary = "aider", args = ["--message", "{prompt}", "--yes-always"], read_only_args = ["--dry-run"] }
```

## Fields

| Field | Required | Notes |
|---|---|---|
| `id` | yes | Lowercase letters, digits, dashes. Can't be a built-in id (`cursor`, `claude`, `chatgpt`, `grok`) |
| `name` | yes | Shown in the app and to other agents |
| `color` | no | Hex colour for its avatar (default grey) |
| `detect.binary` | yes | Executable name |
| `mcp.kind` | yes | `json-merge` (needs `path` and `key`, a dotted path) or `command` (needs `add` and `remove`) |
| `wake` | no | Without it the agent is inbox-only: it sees questions when it calls `mux_inbox` |
| `wake.args` | yes | Must contain `{prompt}`. May use `{workspace}` and `{ticket}` |
| `wake.read_only_args` | no | Appended unless the room allows edits. Use it to keep wakes read-only |

## Safety

- Arguments are passed as an argv list, **never through a shell**.
- Only `{prompt}`, `{workspace}` and `{ticket}` placeholders are allowed; anything else is rejected when the manifest loads.
- Wakes get the same stripped environment and timeout as built-in providers, and run in the room's folder.

If you think others would use your manifest, send a PR adding it to [`providers/`](https://github.com/agentmuxhq/agentmux-mcp/tree/main/providers) in the repo. If your agent needs more than this (webhooks, its own permissions, a context hook), write a [Rust provider](./adding-a-provider.md).
