#!/usr/bin/env bash
# Builds the `agentmux` CLI and places it where Tauri's `externalBin` expects it:
#   src-tauri/binaries/agentmux-<target-triple>
# The desktop app finds it next to its own executable and links ~/.agentmux/bin/agentmux to it.
set -euo pipefail
cd "$(dirname "$0")/.."

# usage: build-sidecar.sh [dev]   (dev = fast debug build for `tauri dev`)
PROFILE=release; FLAG=--release
if [ "${1:-}" = dev ]; then PROFILE=debug; FLAG=; fi
TRIPLE="${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}"
cargo build $FLAG -p agentmux-cli --target "$TRIPLE"
mkdir -p src-tauri/binaries
cp "target/$TRIPLE/$PROFILE/agentmux" "src-tauri/binaries/agentmux-$TRIPLE"
echo "sidecar: src-tauri/binaries/agentmux-$TRIPLE"
