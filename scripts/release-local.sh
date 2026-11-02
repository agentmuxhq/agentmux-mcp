#!/usr/bin/env bash
# Build, sign and publish a release from this Mac. The updater key never leaves ~/.tauri.
#
#   scripts/release-local.sh 0.1.1            # bump, build both Macs, tag, push, create the GitHub release
#   scripts/release-local.sh 0.1.1 --no-publish   # build + sign only; files land in release/v0.1.1/
#
# Release notes: release-notes/<version>.md if present, otherwise a short default.
# Files published: AgentMux_<v>_{aarch64,x64}.dmg, AgentMux_<v>_{aarch64,x64}.app.tar.gz (+ .sig), latest.json
set -euo pipefail
cd "$(dirname "$0")/.."

V="${1:?usage: scripts/release-local.sh X.Y.Z [--no-publish]}"
PUBLISH=1; [ "${2:-}" = "--no-publish" ] && PUBLISH=0
REPO="agentmuxhq/agentmux-mcp"
KEY="$HOME/.tauri/agentmux-updater.key"; PASS="$HOME/.tauri/agentmux-updater.pass"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3"; exit 1; }
[ -f "$KEY" ] && [ -f "$PASS" ] || { echo "updater key not found in ~/.tauri (agentmux-updater.key / .pass)"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit or stash your changes first"; exit 1; }
[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || { echo "release from main"; exit 1; }
if [ "$PUBLISH" = 1 ]; then
  git ls-remote --exit-code --tags origin "refs/tags/v$V" >/dev/null 2>&1 && { echo "tag v$V already exists"; exit 1; }
fi

# 1. Version bump (a no-op when the files already say $V).
sed -i '' -E "s/^(  \"version\": )\"[^\"]+\"/\1\"$V\"/" src-tauri/tauri.conf.json package.json
sed -i '' -E "s/^version = \"[^\"]+\"/version = \"$V\"/" src-tauri/Cargo.toml
cargo update -w >/dev/null 2>&1 || true
if [ -n "$(git status --porcelain)" ]; then git commit -qam "Release v$V"; fi

# 2. Build and sign both architectures.
export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$PASS")"
OUT="release/v$V"; rm -rf "$OUT"; mkdir -p "$OUT"
for T in aarch64-apple-darwin x86_64-apple-darwin; do
  case "$T" in aarch64-*) A=aarch64; P=darwin-aarch64;; *) A=x64; P=darwin-x86_64;; esac
  echo "==> building $T"
  bun tauri build --target "$T" >/dev/null
  B="target/$T/release/bundle"
  cp "$B/dmg/AgentMux_${V}_${A}.dmg" "$OUT/"
  cp "$B/macos/AgentMux.app.tar.gz" "$OUT/AgentMux_${V}_${A}.app.tar.gz"
  cp "$B/macos/AgentMux.app.tar.gz.sig" "$OUT/AgentMux_${V}_${A}.app.tar.gz.sig"
  echo "$P $A" >> "$OUT/.platforms"
done

# 3. Updater manifest.
NOTES_FILE="release-notes/$V.md"
if [ -f "$NOTES_FILE" ]; then NOTES="$(cat "$NOTES_FILE")"; else NOTES="AgentMux v$V"; fi
export V OUT REPO NOTES
python3 - <<'PY'
import json, os, datetime
v, out, repo, notes = os.environ["V"], os.environ["OUT"], os.environ["REPO"], os.environ["NOTES"]
platforms = {}
for line in open(f"{out}/.platforms").read().split("\n"):
    if not line: continue
    p, a = line.split()
    name = f"AgentMux_{v}_{a}.app.tar.gz"
    platforms[p] = {
        "signature": open(f"{out}/{name}.sig").read().strip(),
        "url": f"https://github.com/{repo}/releases/download/v{v}/{name}",
    }
json.dump({"version": v, "notes": notes, "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"), "platforms": platforms},
          open(f"{out}/latest.json", "w"), indent=2)
PY
rm "$OUT/.platforms"
echo "==> built:"; ls -1 "$OUT"

[ "$PUBLISH" = 1 ] || { echo "(--no-publish: nothing pushed)"; exit 0; }

# 4. Tag, push and publish.
git tag "v$V"
git push origin main "v$V"
INSTALL_NOTE=$'\n\n---\nUnsigned macOS build. On first launch macOS will block it: open **System Settings → Privacy & Security → Open Anyway**, or run `xattr -dr com.apple.quarantine /Applications/AgentMux.app`.\nPick `aarch64` for Apple Silicon (M1 and later) or `x64` for Intel Macs.'
gh release create "v$V" "$OUT"/* --repo "$REPO" --title "AgentMux v$V" --notes "$NOTES$INSTALL_NOTE"
echo "Published: https://github.com/$REPO/releases/tag/v$V"
