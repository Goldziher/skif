---
type: Concept
title: Architecture
x-ai-rulez:
  kind: context
  id: architecture
  metadata:
    priority: high
---

# Architecture

sherif is a Rust CLI that lints package.json files in npm/yarn/pnpm/bun monorepos. It is distributed as a binary via npm (`npm/`) and a GitHub Action (`action/`).

## Layout

- `src/main.rs`, `src/args.rs`: entry point and clap argument parsing; CLI args merge over the `sherif` config in the root `package.json`.
- `src/collect.rs`: discovers workspace packages (`workspaces` or `pnpm-workspace.yaml`) and runs every rule to collect issues.
- `src/packages/`: root and workspace package models, semver parsing (`semversion.rs`).
- `src/rules/`: one module per rule; each defines an issue type implementing the shared issue trait in `rules/mod.rs`. Snapshots live in `src/rules/snapshots/` (insta).
- `src/printer.rs`, `src/json.rs`, `src/plural.rs`: output, JSON preservation (indent and newline style), pluralisation.
- `src/install.rs`: the `--install` helper that runs the detected package manager.
- `fixtures/`: sample monorepos exercised by the tests, one directory per scenario.
- `action/`: TypeScript GitHub Action, bundled to `action/index.js` with `pnpm action` (ncc). Commit the rebuilt bundle with source changes.
- `npm/`: templates for the per-platform npm packages.

## Rules

- Rule names are kebab-case and are the public contract (`--ignore-rule`, config files, docs in README).
- Rules report `Error` or `Warning`; fixable rules also implement the fix path used by `--fix`. Fixes must preserve file indentation and newline style.
