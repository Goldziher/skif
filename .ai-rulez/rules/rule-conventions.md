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

- Every rule has a stable kebab-case ID. IDs are the public contract (`--ignore-rule`, config files,
  docs); changing one is a breaking change that needs a superseding ADR and a changelog entry.
- Every rule is registered in the rule registry, whose constructor rejects duplicate IDs, and is
  documented in the generated rules reference.
- New or changed rules need a fixture under `fixtures/` and an insta snapshot test of the diagnostics; never
  edit `.snap` files by hand.
- Fixers keep the original indentation and newline style of the edited file, and a fix applied then re-linted
  leaves no fixable diagnostic.
- Library crates do no I/O: file access goes through the `FileSystem` trait, and printing, writing and exit
  codes live in the CLI crate.
- Never panic on malformed user input; return a typed error naming the offending path.
- Keep output deterministic: sorted iteration and ordered maps, never `HashMap` iteration order.
