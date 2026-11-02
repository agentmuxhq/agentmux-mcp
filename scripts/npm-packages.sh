#!/usr/bin/env bash
# Assemble the npm packages for one release:
#   scripts/npm-packages.sh <version> <arm64-binary> <x64-binary> <out-dir>
# Produces <out-dir>/cli-darwin-arm64, cli-darwin-x64 and cli, ready for `npm publish`.
set -euo pipefail
abs() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
V="$1"; ARM="$(abs "$2")"; X64="$(abs "$3")"; OUT="$(abs "$4")"
cd "$(dirname "$0")/.."
rm -rf "$OUT"; mkdir -p "$OUT"
REPO='{"type":"git","url":"git+https://github.com/agentmuxhq/agentmux-mcp.git"}'

for A in arm64 x64; do
  D="$OUT/cli-darwin-$A"; mkdir -p "$D/bin"
  SRC="$ARM"; [ "$A" = x64 ] && SRC="$X64"
  cp "$SRC" "$D/bin/agentmux"; chmod 755 "$D/bin/agentmux"
  cat > "$D/package.json" <<JSON
{
  "name": "@agentmuxhq/cli-darwin-$A",
  "version": "$V",
  "description": "AgentMux binary for $A Macs. Install @agentmuxhq/cli instead.",
  "os": ["darwin"],
  "cpu": ["$A"],
  "files": ["bin/agentmux"],
  "license": "Apache-2.0",
  "repository": $REPO,
  "homepage": "https://agentmux.dev"
}
JSON
  printf '# @agentmuxhq/cli-darwin-%s\n\nThe AgentMux binary for %s Macs, installed automatically by [@agentmuxhq/cli](https://www.npmjs.com/package/@agentmuxhq/cli).\n' "$A" "$A" > "$D/README.md"
done

D="$OUT/cli"; mkdir -p "$D/bin"
cp npm/cli/bin/agentmux.js "$D/bin/agentmux.js"; chmod 755 "$D/bin/agentmux.js"
cp npm/cli/README.md "$D/README.md"; cp LICENSE "$D/LICENSE"
cat > "$D/package.json" <<JSON
{
  "name": "@agentmuxhq/cli",
  "version": "$V",
  "description": "Let your AI agents message each other: one local MCP server for Cursor, Claude Code, Codex and Grok bots (macOS).",
  "bin": { "agentmux": "bin/agentmux.js" },
  "files": ["bin/agentmux.js", "README.md", "LICENSE"],
  "os": ["darwin"],
  "engines": { "node": ">=18" },
  "optionalDependencies": {
    "@agentmuxhq/cli-darwin-arm64": "$V",
    "@agentmuxhq/cli-darwin-x64": "$V"
  },
  "keywords": ["mcp", "model-context-protocol", "a2a", "agents", "cursor", "claude-code", "codex", "grok"],
  "license": "Apache-2.0",
  "repository": $REPO,
  "homepage": "https://agentmux.dev",
  "bugs": "https://github.com/agentmuxhq/agentmux-mcp/issues"
}
JSON
echo "npm packages for $V in $OUT"
