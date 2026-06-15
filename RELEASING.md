# Releasing

All workspace crates are published to crates.io together, sharing one version
(`[workspace.package].version`). Releases are **tag-triggered** — pushing a
`vX.Y.Z` tag runs `.github/workflows/release.yml`, which publishes every crate
to crates.io in dependency order.

## The release procedure — always these three steps

1. **Bump the version** in the root `Cargo.toml`:

   ```toml
   [workspace.package]
   version = "X.Y.Z"
   ```

2. **Commit it:**

   ```sh
   git commit -am "release X.Y.Z"
   ```

3. **Tag and push** (the tag must be `v` + the exact version from step 1):

   ```sh
   git tag vX.Y.Z && git push origin master vX.Y.Z
   ```

That's it. The `release` workflow publishes all 8 crates
(`relais-core` → adapters + `relais-llm-fallback` → `relais-server` →
`relais-cli`) to crates.io. After it finishes, `cargo install relais-cli`
installs the new version for everyone.

## Rules

- **Never publish by hand** (`cargo publish` locally) for a normal release — go
  through the tag so every crate is released together, in order, from CI.
- **The tag must match the version.** The workflow fails if `vX.Y.Z` does not
  equal `[workspace.package].version`. Bump first, tag second.
- **Versions are immutable.** A published version cannot be reused — only
  yanked. Always bump to a fresh number; never re-tag an existing version.
- **Bump all crates together.** They share the workspace version, so every
  release moves all 8 crates to the same `X.Y.Z` even if only one changed.

## Requirements

- Repo secret `CARGO_REGISTRY_TOKEN` — a crates.io API token with the
  `publish-update` scope. Set under
  *Settings → Secrets and variables → Actions*. Rotate it on crates.io if it is
  ever exposed, then update the secret.

## First publish (already done at 0.1.0)

The very first publish of a brand-new crate hits the crates.io *new-crate* rate
limit (~1 new crate per several minutes). Version **updates** (every release
after the first) are not subject to that limit, so tag-triggered releases run
fast. The workflow retries through HTTP 429 either way.
