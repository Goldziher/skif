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

Tools (poly, adrlint, Node, pnpm, actionlint) are declared in `mise.toml` and locked in `mise.lock`; their
versions follow the xberg-io/toolchains baseline in `toolchains.toml`. Rust is installed by `rustup` locally and
by `dtolnay/rust-toolchain` in CI. Shared tasks (`governance:*`) come from `mise-tasks/shared.toml`, which is
generated; do not edit it.

- Set up (tools, crates, git hooks): `mise run setup`
- Build: `mise run build`
- Test: `mise run test` (`mise run test:verbose` shows output)
- Lint: `mise run lint` (clippy with `-D warnings`, then `poly lint`)
- Format: `mise run format`, verify with `mise run format:check`
- Advisories and licenses: `mise run deny` (needs `cargo-deny` installed)
- Architecture decision records: `mise run adr:lint`
- Full gate before pushing: `mise run check`
- Update or upgrade dependencies: `mise run update` / `mise run upgrade`
- Regenerate or check AI governance outputs: `mise run governance:generate` / `mise run governance:check`
- Review snapshot changes: `cargo insta review`
