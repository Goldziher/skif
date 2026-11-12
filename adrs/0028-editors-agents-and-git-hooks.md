---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Editors, Agents and Git Hooks: `skif mcp`, `skif lsp`, `--watch`, the Agent Skill and the Hooks

## Context and Problem Statement

skif's findings are most useful where code is written: in the editor while typing, in a coding agent before it
changes code, and in a pre-commit hook right after a manifest was edited. Each of these places has its own protocol
or discovery mechanism: the Model Context Protocol for agents, the Language Server Protocol for editors, Agent Skills
and plugin marketplaces for agent knowledge, and `.pre-commit-hooks.yaml`, poly catalogs and Lefthook for hooks.

How does skif integrate with editors, agents and hook managers, without new crates, protocol frameworks or a second
product to release?

## Decision Drivers

* Each integration is a thin loop around what the CLI already does, with the same JSON documents
  ([ADR 0022](0022-reporting-and-output-formats.md))
* No new crate, no protocol dependency and no licence exception
* The library stays I/O-free ([ADR 0003](0003-library-api-and-embedding.md)); servers and watchers live in the CLI
* A fix in the editor produces the same edits as `skif --fix`, and unsafe fixes and file deletion stay explicit
* Hook users need neither a Rust toolchain nor Node
* One copy of the agent skill, installable from GitHub with no build step, whose version moves with every release

## Considered Options

* Hand-rolled JSON-RPC servers and a polling watcher in `skif-cli`, a hand-written skill and manifests at the
  repository root, and a script-language pre-commit launcher with a poly catalog
* MCP and LSP framework crates, and `notify` for watching
* An editor extension and a generated agent plugin bundle
* `language: rust` or `language: node` pre-commit hooks

## Decision Outcome

Chosen option: "Hand-rolled JSON-RPC servers and a polling watcher in `skif-cli`, a hand-written skill and manifests
at the repository root, and a script-language pre-commit launcher with a poly catalog", because the protocol surface
skif needs is a few messages that `serde_json` handles directly, every installer reads the same skill directory, and
the launcher brings the binary without Rust or Node.

### `skif mcp`

* An MCP server on stdio: one JSON-RPC message per line, logs on stderr only. It answers `initialize`, `ping`,
  `tools/list` and `tools/call`, and negotiates the protocol version, answering with the client's version when it
  supports it and with the newest it supports otherwise.
* Tools: `lint`, `explain_usage` and `list_rules`. Each returns the document the matching command prints with
  `--format json`, as `structuredContent` and as text. A tool that cannot run returns `isError: true` with the error
  document ([ADR 0024](0024-cli-contract.md)); an unknown method, a malformed request or batch, and an unknown tool or
  bad argument are the JSON-RPC errors `-32601`, `-32600` and `-32602`. A line has a size limit and is skipped past
  without being buffered.
* Calls run one at a time and in order. The server never writes a file: fixing stays a CLI action the agent runs.

### `--watch`

* `skif --watch` lints, then lints again when a file below the root changes. It polls the tree, at a fixed interval
  or twice the last scan time when that is longer, and starts a run once the tree has been quiet for a short debounce,
  so a checkout causes one run.
* `node_modules`, `.git` and `target` are never entered, `.gitignore` is honoured, and the run's own report files and
  a `--cache-location` below the watched directory are left out of the scan.
* With `--format json` stdout holds one document per run. `--watch` conflicts with `--fix`, and exit codes have no
  meaning while it runs; an unreadable configuration is reported and the loop keeps watching.
* It polls rather than using `notify`, because `notify`'s CC0 licence is not on the `deny.toml` allow-list and its
  platform back ends would strain the duplicate-version ban; polling behaves the same on every platform and on
  network file systems.

### `skif lsp`

* An LSP server on stdio with `Content-Length` framing and UTF-16 positions. A request before `initialize` is
  `-32002`; a frame the server cannot read past ends the process with exit code 1, since the next message's start is
  unknown.
* Full document sync. The server keeps each open document's text and version and lints through a `BufferFs` overlay
  of the library ([ADR 0003](0003-library-api-and-embedding.md)), so unsaved manifests, configuration and sources are
  linted as typed. A buffer whose file does not exist on disk is ignored until it is saved.
* It lints a document's root a short debounce after the last change, and at once on open and save. It publishes
  diagnostics per file, with the rule id as `code`, and an empty list for a file whose findings are gone.
* Code actions are planned by `apply_fixes` from the last lint, as `skif --fix --dry-run` plans them: one `quickfix`
  per fix alternative (an unsafe one labelled "(unsafe)" and never preferred; choosing it is the consent `--fix-unsafe`
  stands for), and `source.fixAll.skif` with the safe fixes and the `[policy] prefer` alternative.
* A plan is offered only when every file it edits holds the text the last lint read, checked by content hash; for an
  open document the edit carries its version when the client supports versioned edits, so a client refuses it after
  further typing. A plan that creates or deletes a file is always withheld.
* `workspace/didChangeWatchedFiles` and cancelling a running lint are not implemented: files changed outside the
  editor are seen on the next lint of their root.

### No editor extension

skif publishes no VS Code extension or other editor plugin, anywhere. Editors run `skif lsp` through their own
language-server client, as the editor guide shows.

### The agent skill and plugin manifests

* `skills/skif/SKILL.md` holds the short loop (lint with JSON output, read the exit code, triage, dry-run the fix,
  fix, verify), with details in `skills/skif/references/` loaded only when needed. It states no rule or plugin
  counts and points to `skif schema` and the website instead of copying reference tables.
* The plugin root is the repository root: `.claude-plugin/marketplace.json` and `.claude-plugin/plugin.json`,
  `.codex-plugin/plugin.json` and `.agents/plugins/marketplace.json`. The `skills` installer finds `skills/skif` on
  its own. Nothing of the repository's own agent configuration ships.
* The MCP server is declared inline in the plugin manifests as `npx -y skif-cli@<version> mcp`, so the plugin needs
  Node and npm but no install of skif and runs the release it documents. It is never declared in a root `.mcp.json`,
  which would be the project's own MCP configuration for every developer of skif.
* The icon is `.claude-plugin/icon.png`, the website's mark at 512 pixels.
* The manifest versions and the server's `skif-cli` pin are release surfaces
  ([ADR 0027](0027-distribution-and-release.md)), and the documentation drift checks cover the files under
  `skills/`.

### Git hooks

* `.pre-commit-hooks.yaml` (pre-commit and prek) uses `language: script` with `scripts/pre-commit/run-skif.sh`. The
  launcher takes `SKIF_BINARY` (failing, not falling back, when it is not executable), else `skif` on `PATH`, else
  downloads the pinned release once into a per-version cache, verified against `checksums.txt` and unpacked through a
  rename so concurrent hooks cannot corrupt it.
* Hooks: `skif` runs `--no-js`, and the opt-in `skif-fix` runs `--fix --no-install --no-prompt --no-js`. They trigger
  on `package.json`, `pnpm-workspace.yaml` and the skif configuration files, with `pass_filenames: false`, because skif
  lints a workspace and one manifest's change can create a finding in another. The JS analysis reads every source
  file, so it belongs in CI rather than in a commit; a commit hook runs no package manager. `--fix` is refused when
  `CI` is truthy, so `skif-fix` is local only.
* `poly-hooks.toml` exposes the same hooks with a `system` channel and an `npm` channel pinned to the release
  (`pnpm dlx skif-cli@X.Y.Z`), which makes it a release surface.
* Lefthook needs nothing from the producer: the documentation carries a snippet, and no config file ships.

### Consequences

* Good, because agents get skif's report as structured data, editors get findings as they type and fixes from the
  same plan as the CLI, and hook users get the binary without Rust or Node.
* Good, because the servers add no crate, protocol dependency or licence exception, and watch mode behaves the same
  on every platform.
* Good, because there is one copy of the skill, read by every installer, and a release moves the plugin version and
  the server's pin with every other version surface.
* Bad, because hand-rolled protocol code follows the specifications by hand; a new MCP revision or LSP feature is
  skif's work.
* Bad, because polling costs a scan of the tree every interval, which grows with the repository.
* Bad, because the language server lints the whole root after each burst of typing and answers nothing while it runs,
  and full sync sends the whole document on every change.
* Bad, because VS Code users need a generic language-server client extension instead of a one-click install.
* Bad, because the plugin's server needs Node and npm, and a plugin installed from the default branch between a bump
  and the npm publish starts no server.
* Bad, because the hook launcher needs a POSIX shell (Git Bash on Windows) and network on first use; `SKIF_BINARY` is
  the escape hatch.

### Confirmation

* `crates/skif-cli/tests/mcp.rs`: `a_session_initializes_lists_tools_and_lints`,
  `an_unknown_version_gets_the_newest_one`, `a_tool_that_cannot_run_returns_the_error_document`,
  `protocol_errors_are_json_rpc_errors` and `explain_usage_returns_the_report`.
* `crates/skif-cli/src/watch.rs`: `an_unchanged_tree_never_fires` and `a_burst_fires_once_after_the_quiet_period`;
  `crates/skif-cli/tests/watch.rs`: `a_changed_manifest_is_linted_again`, `watch_refuses_fix` and
  `the_reports_a_run_writes_do_not_start_another_run`.
* `crates/skif-cli/tests/lsp.rs`: `opening_a_manifest_publishes_the_root_diagnostics`,
  `a_malformed_frame_is_reported_on_stderr_and_the_server_exits`,
  `a_safe_fix_is_a_preferred_quick_fix_and_part_of_fix_all`, `an_unsafe_fix_is_labelled_and_left_out_of_fix_all`,
  `an_unsaved_change_is_linted_before_shutdown_is_answered`,
  `a_fix_for_an_unsaved_buffer_edits_the_buffer_until_the_buffer_changes`,
  `closing_a_document_with_unsaved_changes_lints_the_saved_file`,
  `with_document_changes_an_edit_carries_the_document_version` and
  `a_burst_of_changes_is_linted_once_the_root_is_quiet`.
* `crates/skif/tests/module_cache.rs`: `an_overlay_without_buffers_reports_like_its_base` and
  `a_manifest_buffer_reports_like_the_saved_file`.
* `scripts/check-versions.sh` and `scripts/test-release-scripts.sh` cover the plugin manifest versions and the
  server pin; `scripts/test-check-scripts.sh` covers the drift checks for files under `skills/`.
* `scripts/test-install-scripts.sh` tests the launcher's resolution order, a non-executable `SKIF_BINARY`, a
  corrupted archive and a missing checksum; `scripts/test-pre-commit-hook.sh` runs the hooks through pre-commit or
  prek against a fixture with a known violation and a clean one.

## Pros and Cons of the Options

### Hand-rolled JSON-RPC servers and a polling watcher in `skif-cli`, a hand-written skill and manifests at the repository root, and a script-language pre-commit launcher with a poly catalog

* Good, because each piece reuses code the CLI already tests.
* Good, because nothing is generated or synced.
* Bad, because the plugin root is the repository root: a host that loads `.mcp.json`, `hooks/` or `agents/` from a
  plugin root would load such a path if the repository tracked one, so adding one needs this decision revisited.

### MCP and LSP framework crates, and `notify` for watching

* Good, because protocol details and native file events come from maintained code.
* Bad, because they add crates, an async runtime or a licence exception for a few messages.

### An editor extension and a generated agent plugin bundle

* Good, because VS Code users get a one-click install.
* Bad, because an extension is a second product with its own build and marketplace release, and a generated bundle is
  a second copy of the skill to keep in sync.

### `language: rust` or `language: node` pre-commit hooks

* Good, because pre-commit manages the install.
* Bad, because every consumer needs a Rust toolchain and a full compile, or Node, which hook users may not have.
