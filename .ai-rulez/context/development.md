---
type: Concept
title: Development
x-ai-rulez:
  kind: context
  id: development
  metadata:
    priority: high
---

# Development

## Commands

Every tool (Rust stable with clippy and rustfmt, cargo-deny, cargo-edit, cross, poly, Node, pnpm) is declared in `mise.toml` and locked in `mise.lock`. Local development and CI both go through `mise` and its tasks; do not call tools that mise does not provide.

- Set up (tools, dependencies, git hooks): `mise run setup`
- Build: `mise run build`
- Test: `mise run test` (runs `cargo test -- --test-threads=1`; tests share fixture directories)
- Lint: `mise run lint` (clippy with `-D warnings` plus `poly lint`)
- Format: `mise run format`, verify with `mise run format:check`
- Advisories and licenses: `mise run deny`
- Full gate before pushing: `mise run check`
- Rebuild the Action bundle: `mise run action`
- Update or upgrade dependencies: `mise run update` / `mise run upgrade`
- Review snapshot changes: `cargo insta review`

## CI

`.github/workflows/ci.yml` installs tools with `jdx/mise-action` and runs `mise run` tasks: check (cargo check, rustfmt, clippy `-D warnings`), test, deny, poly lint and format, and a freshness check of the committed `action/index.js`. Releases build with `mise run release:build <target>`.

## Versioning

The version lives in both `Cargo.toml` and `package.json`; bump them together.
