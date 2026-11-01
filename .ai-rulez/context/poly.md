---
type: Concept
title: Poly
x-ai-rulez:
  kind: context
  id: poly
  metadata:
    priority: high
---

# poly

poly is the repository's universal linter and formatter, configured in `poly.toml`. Git hooks are installed with `poly hooks install` and run on commit.

- Lint: `poly lint .`
- Check formatting: `poly fmt .`
- Apply formatting: `poly fmt --fix .`
- Apply lint autofixes: `poly lint --fix .`

`poly.toml` excludes `fixtures/**`, `target/**`, `node_modules/**` and lockfiles. Rust formatting and clippy remain authoritative through `cargo fmt` and `cargo clippy`.
