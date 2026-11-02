#!/usr/bin/env bash
# Usage: scripts/release.sh 0.1.1
# Bumps the version everywhere, commits, tags vX.Y.Z and pushes. GitHub Actions does the rest.
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:?usage: scripts/release.sh X.Y.Z}"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit or stash your changes first"; exit 1; }
[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || { echo "release from main"; exit 1; }

sed -i '' -E "s/^(  \"version\": )\"[^\"]+\"/\1\"$V\"/" src-tauri/tauri.conf.json package.json
sed -i '' -E "s/^version = \"[^\"]+\"/version = \"$V\"/" src-tauri/Cargo.toml crates/agentmux-cli/Cargo.toml
cargo update -w >/dev/null 2>&1 || true

git commit -am "Release v$V"
git tag "v$V"
git push origin main "v$V"
echo "Tagged v$V. Watch it at: https://github.com/agentmuxhq/agentmux-mcp/actions"
