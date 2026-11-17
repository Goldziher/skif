---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# Editors, Agents and Git Hooks

## Context and Problem Statement

skif's findings are most useful where code is written: in the editor while typing, in a coding agent before it changes
code, and in a pre-commit hook right after a manifest was edited. Each place has its own protocol or discovery
mechanism: the Model Context Protocol for agents, the Language Server Protocol for editors, Agent Skills and plugin
marketplaces for agent knowledge, and `.pre-commit-hooks.yaml`, poly catalogs and Lefthook for hooks.

How does skif integrate with editors, agents and hook managers without new crates, protocol frameworks or a second
product to release?

## Decision Drivers

* Each integration is a thin loop around what the CLI already does, with the same JSON documents
  ([ADR 0012](0012-cli-contract-and-reporting.md))
* No new crate, no protocol dependency and no licence exception; servers and watchers live in the CLI, the library
  stays I/O-free ([ADR 0002](0002-crates-library-api-and-upstream-code.md))
* A fix in the editor produces the same edits as `skif --fix`, and unsafe fixes and file deletion stay explicit
* Hook users need neither a Rust toolchain nor Node
* One copy of the agent skill, installable with no build step, whose version moves with every release

## Considered Options

* Hand-rolled JSON-RPC servers and a polling watcher in `skif-cli`, a hand-written skill and manifests at the repository
  root, and a script-language pre-commit launcher with a poly catalog
* MCP and LSP framework crates and `notify` for watching, because they add crates, an async runtime or a licence
  exception for a few messages
* An editor extension and a generated agent plugin bundle, because an extension is a second product with its own
  release, and a generated bundle a second copy of the skill
* `language: rust` or `language: node` pre-commit hooks, because every consumer would need a Rust toolchain and a
  compile, or Node

## Decision Outcome

Chosen option: "Hand-rolled JSON-RPC servers and a polling watcher in `skif-cli`, a hand-written skill and manifests at
the repository root, and a script-language pre-commit launcher with a poly catalog", because the protocol surface skif
needs is a few messages that `serde_json` handles directly, every installer reads the same skill directory, and the
launcher brings the binary without Rust or Node.

### `skif mcp`

* An MCP server on stdio: one JSON-RPC message per line, logs on stderr only. It negotiates the protocol version,
  answering with the client's when it supports it and with its newest otherwise.
* Its tools are `lint` (which also plans fixes as `--fix --dry-run` does), `explain_usage`, `config` and
  `list_rules`, each standing for one command. An argument is the flag of the same name: the server writes the
  arguments as that command line, parses it with the CLI's own clap definition and runs the command's code, so a tool
  returns exactly the document the command prints with `--format json`, as structured content and as compact JSON
  text. A command that exits `2` gives `isError: true` with its error document (or its report, when roots of a
  multi-root run failed); protocol faults are the standard JSON-RPC errors. An oversized line is skipped without being
  buffered.
* Commands that write files, use the network or describe static data (`init`, `migrate`, `baseline`, `update`,
  `format`, `schema`, `list`, `json`) have no tool, and neither have flags that only change the exit code, colour,
  stderr or configuration overrides; the parity tests name each with its reason.
* Calls run one at a time and in order. The server never writes a file: fixing stays a CLI action the agent runs.

### `skif lsp`

* An LSP server on stdio with `Content-Length` framing, UTF-16 positions and full document sync. It lints through a
  `BufferFs` overlay of the library ([ADR 0002](0002-crates-library-api-and-upstream-code.md)), so unsaved manifests,
  configuration and sources are linted as typed. A frame it cannot read past ends the process, since the next
  message's start is unknown.
* It lints a document's root a short debounce after the last change, and at once on open and save, and publishes
  diagnostics per file with the rule id as `code`.
* Code actions are planned by `apply_fixes` from the last lint, as `skif --fix --dry-run` plans them: one quick fix per
  alternative (an unsafe one labelled and never preferred; choosing it is the consent `--fix-unsafe` stands for) and a
  `source.fixAll.skif` action with the safe fixes, taking the alternative `[policy] prefer` names where there are
  several.
* A plan is offered only when every file it edits holds the text the last lint read, checked by content hash; an edit
  of an open document carries its version where the client supports it. A plan that creates or deletes a file is
  always withheld.

### `--watch`

* `skif --watch` lints, then lints again when a file below the root changes. It polls the tree and starts a run once
  the tree has been quiet for a short debounce, so a checkout causes one run. It never enters `node_modules`, `.git`
  or `target`, honours `.gitignore`, and leaves the run's own report and cache files out of the scan.
* It polls rather than using `notify`, because `notify`'s licence is not on the `deny.toml` allow-list and its platform
  back ends would strain the duplicate-version ban; polling behaves the same on every platform and on network file
  systems.
* `--watch` conflicts with fixing, exit codes have no meaning while it runs, and an unreadable configuration is
  reported while the loop keeps watching.

### Long-lived hosts run untyped

`skif mcp`, `skif lsp` and `--watch` analyse without types, for the reason
[ADR 0010](0010-type-aware-analysis-on-tsrs.md) states, so their member findings are name-based.

### No editor extension

skif publishes no VS Code extension or other editor plugin. Editors run `skif lsp` through their own language-server
client, as the editor guide shows.

### The agent skill and plugin manifests

* `skills/skif/SKILL.md` holds the short loop (lint with JSON output, read the exit code, triage, dry-run the fix, fix,
  verify), with details in `skills/skif/references/` loaded only when needed. It states no rule or plugin counts and
  points to `skif schema` and the website instead of copying reference tables.
* The plugin root is the repository root: `.claude-plugin/`, `.codex-plugin/` and `.agents/plugins/` hold the
  manifests and marketplaces, and the `skills` installer finds `skills/skif` on its own. Nothing of the repository's
  own agent configuration ships.
* The MCP server is declared inline in the plugin manifests as `npx -y @polylint/skif@<version> mcp`, so the plugin
  needs Node and npm but no install of skif and runs the release it documents. It is never declared in a root
  `.mcp.json`, which would configure every developer of skif.
* The manifest versions and the server's pin are release surfaces
  ([ADR 0015](0015-distribution-release-and-supported-versions.md)).

### Git hooks

* `.pre-commit-hooks.yaml` (pre-commit and prek) uses `language: script` with `scripts/pre-commit/run-skif.sh`. The
  launcher takes `SKIF_BINARY` (failing, not falling back, when it is not executable), else `skif` on `PATH`, else
  downloads the pinned release once into a per-version cache, verified against `checksums.txt` and its build
  attestation and unpacked through a rename so concurrent hooks cannot corrupt it.
* The hooks run without the JS analysis, which reads every source file and belongs in CI, and the opt-in fixing hook
  runs no package manager. They trigger on manifests, workspace files and skif's configuration with
  `pass_filenames: false`, because one manifest's change can create a finding in another. The fixing hook is local
  only, since fixes are refused in CI.
* `poly-hooks.toml` exposes the same hooks with a `system` channel and an `npm` channel pinned to the release
  (`pnpm dlx @polylint/skif@X.Y.Z`), which makes it a release surface.
* Lefthook needs nothing from the producer: the documentation carries a snippet, and no config file ships.

### Consequences

* Good, because agents get skif's report as structured data, editors get findings as they type and fixes from the same
  plan as the CLI, and hook users get the binary without Rust or Node.
* Good, because the servers add no crate, protocol dependency or licence exception, and watch mode behaves the same on
  every platform.
* Good, because there is one copy of the skill, and a release moves the plugin version and the server's pin with every
  other version surface.
* Bad, because hand-rolled protocol code follows the specifications by hand; a new MCP revision or LSP feature is
  skif's work.
* Bad, because member findings in the editor, the agent server and watch mode can differ from those of a typed run.
* Bad, because polling scans the tree every interval, and the language server lints the whole root after each burst of
  typing.
* Bad, because the plugin's server needs Node and npm, and the hook launcher a POSIX shell and the network on first use;
  `SKIF_BINARY` is the escape hatch.
* Bad, because a host that loads other paths from a plugin root would load them from the repository root, so adding
  one needs this decision revisited.

### Confirmation

* `crates/skif-cli/tests/mcp.rs`, `crates/skif-cli/tests/lsp.rs` and `crates/skif-cli/tests/watch.rs`, with
  `crates/skif-cli/src/watch.rs`, cover the servers and the watcher. `crates/skif-cli/src/mcp/parity.rs` checks the
  tools against the clap definition (every command and every flag of a tool's command has an argument or a reason, and
  each argument parses as its flag), and `crates/skif-cli/tests/mcp.rs` compares each tool's result with the CLI's
  JSON output for the same input.
* `crates/skif-cli/src/lint.rs` checks that `--watch` and the servers analyse without types.
* `crates/skif/tests/module_cache.rs` checks that a buffer overlay reports like the saved file.
* `scripts/check-versions.sh` and `scripts/test-release-scripts.sh` cover the plugin manifest versions, the server pin
  and the poly hook channel; `scripts/test-check-scripts.sh` covers the drift checks for files under `skills/`.
* `scripts/test-install-scripts.sh` tests the launcher's resolution order and its verification, and
  `scripts/test-pre-commit-hook.sh` runs the hooks through pre-commit or prek against a failing and a clean fixture.
