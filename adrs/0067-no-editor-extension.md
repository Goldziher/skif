---
status: accepted
date: 2026-11-08
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0056", "0064", "0066"]
---

# No Editor Extension

## Context and Problem Statement

[ADR 0056](0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md) put a VS Code extension
out of scope for this repository, as a separate package that could launch `skif lsp`.
[ADR 0064](0064-code-actions-in-the-language-server.md) and
[ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md) repeat that the extension is deferred
or "stays in a separate repository", which reads as planned work. Issue
[#44](https://github.com/Goldziher/skif/issues/44) kept it as its last open item.

Does skif ship an editor extension?

## Decision Drivers

* An extension is a second product: a TypeScript package with its own build, marketplace publishing and versioning
* `skif lsp` already speaks the Language Server Protocol, which editors start through their own client support

## Considered Options

* Ship no editor extension
* Build a VS Code extension in a separate repository

## Decision Outcome

Chosen option: "Ship no editor extension", because the language server covers editor integration and an extension
adds a release channel to maintain.

* skif publishes no VS Code extension or other editor plugin, in this repository or another.
* Editors integrate through `skif lsp`, configured in the editor's own language-server client settings, as the
  editor guide shows.

### Consequences

* Good, because there is no second package, build or marketplace release to keep in step with the CLI.
* Bad, because VS Code users need a generic language-server client extension to run `skif lsp`, instead of a
  one-click install.
