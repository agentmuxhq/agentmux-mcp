#!/usr/bin/env bash
# Print the Homebrew formula for one release:
#   scripts/brew-formula.sh <version> <arm64-tarball-sha256> <x64-tarball-sha256>
set -euo pipefail
V="$1"; ARM="$2"; X64="$3"
BASE="https://github.com/agentmuxhq/agentmux-mcp/releases/download/v$V"
cat <<RB
class Agentmux < Formula
  desc "Let your AI agents message each other: local MCP server for agent-to-agent asks"
  homepage "https://agentmux.dev"
  version "$V"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "$BASE/agentmux-cli-$V-darwin-arm64.tar.gz"
      sha256 "$ARM"
    end
    on_intel do
      url "$BASE/agentmux-cli-$V-darwin-x64.tar.gz"
      sha256 "$X64"
    end
  end

  def install
    bin.install "agentmux"
  end

  def caveats
    <<~EOS
      Set up AgentMux (connects your agents and starts it at login):
        agentmux init
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/agentmux --version")
  end
end
RB
