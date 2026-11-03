# Releasing

Maintainers only. One command does it:

```sh
scripts/release.sh 0.3.0
```

It bumps the version in `src-tauri/tauri.conf.json`, `package.json`, `src-tauri/Cargo.toml` and `crates/agentmux-cli/Cargo.toml`, commits, tags `v0.3.0` and pushes. `.github/workflows/release.yml` then:

1. runs the checks;
2. builds and signs the app for Apple Silicon and Intel (`.dmg` plus updater bundles and `latest.json`);
3. builds the CLI tarballs;
4. publishes the GitHub release, which existing apps pick up as an update;
5. publishes the npm packages (`npm-publish.yml`, Trusted Publishing);
6. updates the Homebrew formula in `agentmuxhq/homebrew-tap`.

The website (and these docs) deploys separately from the `agentmux-website` repo: `bun run build && bunx wrangler pages deploy dist --project-name agentmux --branch main`.
