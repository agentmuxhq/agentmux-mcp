#!/usr/bin/env node
// Runs the native AgentMux binary for this Mac (shipped in @agentmux/cli-darwin-arm64 / -x64).
// `npx @agentmux/cli` with no arguments sets AgentMux up (`agentmux init`).
"use strict";
const { spawnSync } = require("node:child_process");

if (process.platform !== "darwin") {
  console.error("AgentMux runs on macOS for now. Windows and Linux are coming: https://agentmux.dev");
  process.exit(1);
}
const arch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x64" : null;
if (!arch) {
  console.error(`AgentMux doesn't have a build for ${process.arch} Macs.`);
  process.exit(1);
}
let bin;
try {
  bin = require.resolve(`@agentmux/cli-darwin-${arch}/bin/agentmux`);
} catch {
  console.error(`The AgentMux binary for ${arch} Macs is missing. Reinstall with: npm install -g @agentmux/cli`);
  process.exit(1);
}
let args = process.argv.slice(2);
if (args.length === 0 && process.env.npm_command === "exec") args = ["init"];
const r = spawnSync(bin, args, { stdio: "inherit" });
if (r.error) {
  console.error(r.error.message);
  process.exit(1);
}
process.exit(r.status ?? 1);
