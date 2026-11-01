---
type: Decision
title: Rule Conventions
x-ai-rulez:
  kind: rule
  id: rule-conventions
  metadata:
    priority: high
---

# Rule Conventions

- Every lint rule lives in its own module under `src/rules/`, is registered in `src/rules/mod.rs` and `src/collect.rs`, and is documented in the README rules table.
- New or changed rules need a fixture in `fixtures/` and an insta snapshot test; never edit `.snap` files by hand.
- Fixers must keep the original indentation and newline style of the edited `package.json`.
- Never panic on malformed user input; return `anyhow` errors with the offending path.
- Keep output deterministic: use `IndexMap` and sorted iteration, never `HashMap` iteration order.
- When `action/index.ts` changes, regenerate and commit `action/index.js`.
