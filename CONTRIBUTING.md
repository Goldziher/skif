# Contributing to skif

Thanks for helping out. skif is a Rust dependency linter for JavaScript and TypeScript monorepos. Its
diagnostics gate other people's CI, so a false positive or an unstable rule ID is a regression.

Read the records in [`adrs/`](adrs/) before your first change. They are short and they explain most of
the design.

## Setup

```bash
git clone https://github.com/Goldziher/skif.git
cd skif
mise install
mise run setup
```

`mise run setup` installs the pinned tools, fetches the crates and installs the git hooks.

## The gate

```bash
mise run check
```

It runs the format check, the lint (clippy with warnings denied, then poly) and the tests. Hooks run
the same checks on commit; do not commit with `--no-verify`. Commits are conventional (`feat:`, `fix:`,
`perf:`, `docs:`, `refactor:`, `test:`, `build:`, `ci:`, `chore:`).

`mise run deny` checks advisories and licenses; it needs `cargo-deny` installed locally.

## Adding a rule

1. Give the rule a stable kebab-case ID. IDs are the public contract.
2. Add a fixture under `fixtures/` and a snapshot test for the diagnostics. Never edit `.snap` files by hand.
3. Fixers must keep the original indentation and newline style of the edited file.
4. Document the rule.

## Architecture decisions

Substantive design changes get an ADR in [`adrs/`](adrs/), linted with [adrlint](https://github.com/Goldziher/adrlint)
(`mise run adr:lint`). Amend an existing ADR in place when a decision evolves; write a new one only when a
decision is replaced.

## Releasing

To be defined.

## AI governance

Run `mise install`, then `mise run governance:hooks:install`. The governance tasks run the latest
ai-rulez through `npx -y ai-rulez@latest`, which needs the Node that mise installs.

Edit `.ai-rulez/` sources, then run `mise run governance:generate` and `npx -y ai-rulez@latest lock`.
`mise run governance:check` verifies the generated outputs are fresh.
