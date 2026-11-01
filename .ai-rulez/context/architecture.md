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

skif is a Rust dependency linter for npm, yarn, pnpm and bun monorepos. Authoritative design decisions
live in `adrs/`; read them before changing structure.

## Current state

The code is still a single crate: `src/main.rs`, `src/args.rs`, `src/collect.rs`, `src/packages/`,
`src/rules/` (one module per rule, insta snapshots in `src/rules/snapshots/`), `src/printer.rs`,
`src/json.rs`, `src/install.rs`, with sample monorepos in `fixtures/`. It is being replaced by the
workspace below.

## Target: library-first workspace

All crates publish to crates.io as `skif-*`. `skif` is the embeddable facade; the binary lives in
`skif-cli`.

| Crate | Responsibility |
| --- | --- |
| `skif-core` | Plain data: diagnostics, spans, fixes, rule metadata, reports. No I/O. |
| `skif-specifier` | Vendored syncpack-specifier, thread-safe, with attribution headers. |
| `skif-config` | Typed config (TOML + builder), discovery, per-rule options, JSON Schema. |
| `skif-edit` | Format-preserving edits from fix operations (package.json, pnpm-workspace.yaml). |
| `skif-workspace` | Package manager and workspace discovery, packages, catalogs, dependency instances, `FileSystem` trait. |
| `skif-lint` | `Rule`, `RuleProvider`, `Registry`, `LintContext`, central ignores and severity, deterministic order. |
| `skif-rules` | `manifest/` (ported rules) and `policy/` (version groups, semver groups, catalogs, workspace protocol). |
| `skif-js-syntax` | The oxc boundary: file to module info. No oxc types exported. |
| `skif-js-plugins` | Plugin trait and declarative plugin table. |
| `skif-js` | Module graph and JavaScript analysis rules. |
| `skif-report` | human, json, sarif and github renderers. |
| `skif-cli` | The `skif` binary: clap, terminal output, file writes, exit codes. |
| `skif-testing` | Dev-only fixture and redaction helpers (`publish = false`). |

Library crates do no I/O beyond the `FileSystem` trait; reading, writing and terminal output belong to
`skif-cli`.

## Rules

Rule IDs are kebab-case and are the public contract (`--ignore-rule`, config files, docs). Rules report
`Error` or `Warning`; fixable rules also emit fix operations. Fixes preserve indentation and newline style.
