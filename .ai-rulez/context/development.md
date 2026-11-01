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

Tool versions (Node, pnpm, poly) are pinned in `mise.toml` and locked in `mise.lock`; `toolchains.toml` names the xberg-io/toolchains baseline. Rust follows the stable channel.

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

`.github/workflows/ci.yml` runs check, test, clippy (`-D warnings`) and rustfmt. All four must pass.

## Versioning

The version lives in both `Cargo.toml` and `package.json`; bump them together.
