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

- Build: `cargo build --locked`
- Test: `cargo test -- --test-threads=1` (tests share fixture directories; do not run in parallel)
- Lint: `cargo clippy --tests -- -D warnings`
- Format: `cargo fmt --all`
- Rebuild the Action bundle: `pnpm install && pnpm action`
- Review snapshot changes: `cargo insta review`

## CI

`.github/workflows/ci.yml` runs check, test, clippy (`-D warnings`) and rustfmt. All four must pass.

## Versioning

The version lives in both `Cargo.toml` and `package.json`; bump them together.
