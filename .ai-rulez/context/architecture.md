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

## Crates

All crates publish to crates.io as `skif-*`. `skif` is the embeddable facade; the binary lives in
`skif-cli`.

| Crate | Responsibility |
| --- | --- |
| `skif-core` | Plain data: diagnostics, spans, fixes, rule metadata, reports, the `FileSystem` trait. No I/O. |
| `skif-specifier` | Vendored syncpack-specifier, thread-safe, with attribution headers. |
| `skif-config` | Typed config (TOML + builder), discovery, per-rule options, JSON Schema. |
| `skif-edit` | Format-preserving edits from fix operations (package.json, pnpm-workspace.yaml). |
| `skif-workspace` | Package manager and workspace discovery, packages, catalogs, dependency instances. |
| `skif-lint` | `Rule`, `RuleProvider`, `Registry`, `LintContext`, central ignores and severity, deterministic order. |
| `skif-rules` | `manifest/` (ported rules) and `policy/` (version groups, semver groups, catalogs, workspace protocol). |
| `skif-js-syntax` | The oxc boundary: file to module info. No oxc types exported. |
| `skif-js-plugins` | The plugin table (entry patterns, config readers) and script parsing. |
| `skif-js` | Module graph and the 14 JS analysis rules (`RuleProvider`). |
| `skif-report` | human, json, sarif and github renderers. |
| `skif` | The facade: discover, lint and plan fixes without I/O side effects. |
| `skif-cli` | The `skif` binary: clap, terminal output, file writes, exit codes. |
| `skif-node` | The napi-rs addon behind the npm package; not published to crates.io. |

Sample monorepos live in the top-level `fixtures/` (`e2e`, `policy`) and in each crate's `tests/fixtures` directory.

Library crates do no I/O beyond the `FileSystem` trait; reading, writing and terminal output belong to
`skif-cli`.

## Rules

Rule IDs are kebab-case and are the public contract (`--ignore-rule`, config files, docs). Rules report
diagnostics at a severity (`error`, `warning` or `info`; a rule's configured level is `off`, `info`, `warn` or `error`); fixable rules also emit fix operations. Fixes preserve indentation and newline style.
