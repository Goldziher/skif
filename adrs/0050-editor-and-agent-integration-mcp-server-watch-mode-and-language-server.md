---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0016"]
---

# Editor and Agent Integration: an MCP Server, Watch Mode and a Language Server in the CLI

## Context and Problem Statement

skif runs from a terminal, a git hook or CI. knip, whose analysis skif covers, also runs inside the tools people
write code with: it has a language server, a VS Code extension, an MCP server (`@knip/mcp`), a `--watch` mode and
a session API. An agent that edits a monorepo wants to ask "what is wrong now" and "why is this export used"
without parsing terminal text, and a person editing `package.json` wants the finding where they type, not after
the next commit.

[ADR 0003](0003-library-first-embedding.md) keeps the library free of I/O, and [ADR 0002](0002-crate-layout-and-publishing.md) puts terminal concerns in `skif-cli`.
A server reads stdin, writes stdout and watches files, so it is I/O by definition. The question is which of these
integrations skif ships, where the code lives, and what each one promises.

## Decision Drivers

* An agent must get skif's findings as data, with the same report the `--format json` contract describes
* The library stays I/O-free; the servers are hosts of it, like the CLI
* No new crate unless a boundary needs it, and no dependency that needs a change of the license policy in `deny.toml`
* Every server has to be testable without an editor or an agent: in-process, over in-memory streams
* `--cache` matters less, because a run is already fast

## Considered Options

* Three CLI entry points: `skif mcp`, `--watch` and `skif lsp`, hand-rolled JSON-RPC over stdio, in `skif-cli`
* A new `skif-server` crate holding the MCP and LSP servers
* The `rmcp`, `tower-lsp` or `lsp-server` crates instead of hand-rolled protocol code
* `notify` for watch mode instead of polling
* A VS Code extension in this repository

## Decision Outcome

Chosen option: "Three CLI entry points: `skif mcp`, `--watch` and `skif lsp`, hand-rolled JSON-RPC over stdio, in
`skif-cli`", because each is a thin loop around functions the CLI already has (`lint_to_json`, the explain-usage
report, the rule list), none needs a crate boundary of its own, and the protocol surface skif needs is a few
messages that `serde_json` handles directly.

### `skif mcp`

`skif mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server on the stdio transport: one
JSON-RPC 2.0 message per line on stdin, one per line on stdout, logs on stderr only. It answers `initialize`,
`ping`, `tools/list` and `tools/call`, accepts the `notifications/initialized` and `notifications/cancelled`
notifications, and answers any other request with the JSON-RPC error `-32601`. It negotiates the protocol
version: it answers with the version the client asked for when it supports it (`2025-11-25`, `2025-06-18`,
`2025-03-26` or `2024-11-05`), else with `2025-11-25`. A batch (a JSON array) is answered with `-32600`, as
batches are not part of the current revision.

It offers three tools. Each returns the same JSON document the matching command prints with `--format json`, as
`structuredContent` and as one `text` content item:

| Tool | Arguments | Result |
| --- | --- | --- |
| `lint` | `path` (default `.`), `recursive`, `rules` (only these rule IDs run), `production`, `no_js` | the lint report of [ADR 0012](0012-reporting.md) |
| `explain_usage` | `file`, `export`, `path` (default `.`), `production` | the `report` of `skif explain-usage` |
| `list_rules` | none | the document of `skif rules --format json` |

A relative `path` is relative to the server's working directory, and a relative `file` to `path`. A tool that
cannot run (no workspace, an invalid `skif.toml`, an unknown rule ID, a file outside the module graph) returns a
result with `isError: true` and the `error` document of [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), as the protocol asks, so the agent sees
the reason; an unknown tool or a malformed argument is the JSON-RPC error `-32602`. A lint that finds problems is
not an error. The server never writes a file: fixing stays a CLI action the agent runs, and `fix_plan` is not
offered until a dry-run plan has a JSON form of its own.

### `--watch`

`skif --watch [PATH]` lints, then lints again whenever a file below the root changes, until it is interrupted. It
polls: every 200 milliseconds, or twice as long as the last scan took when that is longer, it lists the files
below the directory the report's paths are relative to, with their size and modification time, and compares the
list with the previous one. `node_modules`, `.git` and `target` are never entered, and `.gitignore` files are
honoured, so build output does not trigger a run. A run starts once the tree has been quiet for 100 milliseconds,
so a checkout or a formatter that writes many files causes one run, not one per file.

With `--format human` on a terminal the screen is cleared before each run. With `--format json` stdout holds one
JSON document per run, each on its own lines; this amends the "exactly one JSON document" statement of
[ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) for `--watch` only. `--watch` conflicts with `--fix` and `--fix-unsafe`, because a fix
writes files the watcher would see and the install step would run on every save. Exit codes have no meaning while
the loop runs; a configuration that cannot be read is reported and the loop keeps watching, so the user can fix
it.

Polling instead of `notify`: `notify` is the maintained cross-platform watcher, but it is licensed CC0-1.0, which
the `deny.toml` allow-list does not contain, and it brings platform back ends (inotify, kqueue, FSEvents,
`ReadDirectoryChangesW`) whose versions the `multiple-versions = "deny"` ban would have to accommodate. Polling
needs no new dependency (it walks with `ignore`, already in the workspace), behaves the same on every platform,
and works on network and container file systems where native events are missing. Its cost grows with the number
of files; the interval stretches with the scan time to bound it. A later record can move to `notify` if the
license policy admits CC0-1.0.

### `skif lsp`

`skif lsp` is a minimal [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) server
over stdio with `Content-Length` framing. It answers `initialize` with full-document sync on open and save only,
and `shutdown`, and exits on `exit`. On `textDocument/didOpen` and `textDocument/didSave` of a file it lints the
root that contains the file and publishes `textDocument/publishDiagnostics` for every file that has findings,
and an empty list for a file whose findings are gone. Diagnostics carry the rule ID as `code`, `skif` as `source`
and the rule's help as related text. It lints what is on disk; unsaved buffers are not read, which is why
`didChange` is not handled. Code actions from fixes are deferred: they need the fix plan's edits mapped to
document positions, which a later record decides together with a dry-run plan in JSON.

### Not here

A VS Code extension is a separate package with its own release channel (the marketplace); it can launch
`skif lsp`, and is out of scope for this repository. `--cache` is deferred: skif is fast enough that a cache's
invalidation risk is not yet worth it. A session API for embedding hosts already exists as the library facade
([ADR 0003](0003-library-first-embedding.md)).

### Consequences

* Good, because an agent gets skif's report as structured data from a tool call, with no output parsing and the
  same document the CLI contract describes.
* Good, because the servers add no crate, no protocol dependency and no license exception; they are a few hundred
  lines in `skif-cli` over code the CLI already tests.
* Good, because watch mode behaves the same on Linux, macOS, Windows and network file systems.
* Bad, because hand-rolled protocol code has to follow the specifications by hand; a new MCP revision or an LSP
  feature (code actions, pull diagnostics) is skif's work, not a dependency upgrade.
* Bad, because polling costs a scan of the tree every interval, which grows with the repository; native events
  would cost nothing while idle.
* Bad, because the language server lints the saved file, not the buffer, so a finding appears on save, not while
  typing.

### Confirmation

* `crates/skif-cli/tests/mcp.rs` runs `skif mcp` with a scripted stdin and asserts the `initialize` answer and
  version negotiation, the tool list, a `lint` call on a fixture with problems, a `rules` filter, an
  `explain_usage` call, `list_rules`, an `isError` result for a missing workspace, and the JSON-RPC errors for an
  unknown method, an unknown tool and a parse error.
* Unit tests in `crates/skif-cli/src/watch.rs` drive the debouncer with synthetic instants: a burst of changes
  fires once, after the quiet period, and an unchanged tree never fires. An integration test starts
  `skif --watch --format json`, changes a manifest and waits for a second document.
* `crates/skif-cli/tests/lsp.rs` sends framed `initialize`, `didOpen`, `shutdown` and `exit` messages and asserts the
  `publishDiagnostics` notification for the opened manifest.
* `every_long_flag_is_in_the_cli_reference` fails when `--watch` is missing from the CLI reference.

## Pros and Cons of the Options

### Three CLI entry points: `skif mcp`, `--watch` and `skif lsp`, hand-rolled JSON-RPC over stdio, in `skif-cli`

* Good, because the servers reuse the CLI's root resolution, configuration and JSON rendering unchanged.
* Good, because the npm package and `cargo install skif-cli` ship them with no new artefact.
* Bad, because the CLI crate grows two protocol implementations.

### A new `skif-server` crate holding the MCP and LSP servers

* Good, because the protocol code would be separate from the clap front end.
* Bad, because it would depend on `skif-cli` for the functions it serves, or move them, and adds a published crate
  to the list of [ADR 0002](0002-crate-layout-and-publishing.md) and its consistency checks for no consumer but the binary.

### The `rmcp`, `tower-lsp` or `lsp-server` crates instead of hand-rolled protocol code

* Good, because the protocol details would be maintained upstream.
* Bad, because `rmcp` and `tower-lsp` bring an async runtime into a synchronous CLI, and each adds a dependency tree
  for a handful of messages; `lsp-server` is small but covers only the LSP half.

### `notify` for watch mode instead of polling

* Good, because native events cost nothing while idle and arrive at once.
* Bad, because its CC0-1.0 license is outside the `deny.toml` allow-list, and its platform back ends risk the
  duplicate-version ban.

### A VS Code extension in this repository

* Good, because editor users would get skif with one install.
* Bad, because it is a TypeScript package with its own build, marketplace publishing and versioning, which belongs
  in a repository of its own once the language server has users.

## More Information

* Related decisions: [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0012](0012-reporting.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* Issue: [#44](https://github.com/Goldziher/skif/issues/44)
* See the [ADR index](index.md).
