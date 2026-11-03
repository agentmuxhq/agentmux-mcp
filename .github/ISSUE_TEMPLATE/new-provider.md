---
name: New provider
about: Propose (or announce a PR for) support for another agent
title: "Provider: <agent name>"
labels: provider
---

**Agent:** name, link, and whether it runs locally or in the cloud.

**Detection:** how AgentMux can tell it's installed (binary name, app path, config file).

**MCP registration:** where its MCP config lives, or the command that adds a server.

**Wake:** the headless command that takes a prompt and prints an answer, and the flag that keeps it read-only. Or: webhook / inbox only.

**Special needs:** many instances? Reachable from the cloud? A way to show updates in its own chat (a hook)?

**Manifest or Rust?** Could this be a TOML manifest (see docs/custom-agents.md), or does it need a Rust provider (docs/adding-a-provider.md)?

**Verified:** how you checked read-only wakes and install/uninstall leave the user's config intact.
