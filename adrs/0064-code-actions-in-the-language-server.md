---
status: accepted
date: 2026-11-08
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0056"]
---

# Code Actions in the Language Server

## Context and Problem Statement

`skif lsp` publishes diagnostics on open and save, but fixing them takes a trip to the terminal.
[ADR 0056](0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md) deferred code actions
until the fix plan's edits could be mapped to document positions, and did not handle `didChange`, because the
server lints the saved file. Issue #44 asks for the fixes in the editor.

Which fixes does the server offer, how is an unsafe fix presented, and how does it avoid edits whose positions no
longer match the document?

## Decision Drivers

* An action applies the same edits `skif --fix` would; no second edit implementation
* Unsafe fixes and file deletion stay explicit opt-ins ([ADR 0008](0008-fix-and-edit-model.md),
  [ADR 0052](0052-file-deletion-opt-in-fix-types-and-member-removal-fixes.md))
* An edit is never applied at positions that no longer hold the text it was planned against
* The server still lints what is on disk, not the buffer

## Considered Options

* Plan with `apply_fixes` from the last lint and offer quick fixes and a fix-all action, withholding edits to unsaved documents
* Offer only safe fixes
* Lint unsaved buffers so fixes always match the document

## Decision Outcome

Chosen option: "Plan with `apply_fixes` from the last lint and offer quick fixes and a fix-all action, withholding
edits to unsaved documents", because it reuses the CLI's fix plan unchanged and keeps every opt-in the CLI has.

The server keeps the runs of each root's last lint and answers `textDocument/codeAction`, advertising the kinds
`quickfix` and `source.fixAll.skif`. Actions are planned by `skif::apply_fixes`, as `skif --fix --dry-run` plans
them, and each `FileChange::Modify` edit becomes a `TextEdit` in UTF-16 positions.

* Quick fixes: every fix of a diagnostic under the requested range is a `quickfix`, one per alternative. An
  unsafe fix is offered with "(unsafe)" after its title and is never preferred; choosing it is the consent
  `--fix-unsafe` stands for. A safe fix that is its diagnostic's only fix is preferred.
* Fix all: `source.fixAll.skif` applies every safe fix of the document's diagnostics, taking the alternative
  `[policy] prefer` names where there are several, as a non-interactive `skif --fix` does.
* `context.only` is honoured, matching kinds by the dotted hierarchy.

Withheld:

* A plan that creates or deletes a file; deletion stays the CLI's `--allow-remove-files` opt-in.
* A plan that edits a document with unsaved changes, or a file whose contents no longer match the plan's base
  hash, since the edits are positions in the file as linted.

To know which documents have unsaved changes, the server advertises incremental `didChange` (`change: 2`, the
smallest notifications). A change only marks the document dirty; `didOpen` and `didSave` clear the mark, and
`didClose` forgets it. Buffers are still not linted.

`--cache` and the VS Code extension stay deferred, as ADR 0056 decided.

### Consequences

* Good, because a fix in the editor and `skif --fix` produce the same edits from the same plan.
* Good, because unsafe fixes and file deletion stay explicit choices.
* Bad, because a document with unsaved changes gets no fixes until it is saved.
* Bad, because the client sends a notification for every change only so the server can track dirty state.

### Confirmation

* `crates/skif-cli/tests/lsp.rs` asserts that a safe fix is a preferred quick fix whose edit gives the expected
  file and is part of fix-all, that `only` filters the kinds, that an unsafe fix is labelled and left out of
  fix-all, and that a document with unsaved changes gets no fixes until it is saved.
* Unit tests in `crates/skif-cli/src/lsp/actions.rs` cover kind matching and range overlap.

## Pros and Cons of the Options

### Plan with `apply_fixes` from the last lint and offer quick fixes and a fix-all action, withholding edits to unsaved documents

* Good, because no edit logic is duplicated.
* Bad, because the server keeps every root's last runs in memory.

### Offer only safe fixes

* Good, because no fix that changes behaviour is one click away.
* Bad, because an unsafe fix is still a deliberate choice the CLI offers, and the editor would have no way to make it.

### Lint unsaved buffers so fixes always match the document

* Good, because fixes and diagnostics would follow typing.
* Bad, because the library reads files through the `FileSystem` trait and a run covers the whole root; an overlay
  of buffers is a larger change that ADR 0056 left open.

## More Information

* Related decisions: [ADR 0008](0008-fix-and-edit-model.md), [ADR 0052](0052-file-deletion-opt-in-fix-types-and-member-removal-fixes.md), [ADR 0056](0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md)
* Issue: [#44](https://github.com/Goldziher/skif/issues/44)
* See the [ADR index](index.md).
