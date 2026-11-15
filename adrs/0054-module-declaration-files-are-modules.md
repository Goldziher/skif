---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Module Declaration Files Are Modules

## Context and Problem Statement

skif made every declaration file (`.d.ts`, `.d.mts`, `.d.cts`) an entry, because TypeScript loads ambient declarations
without an import. The exports of an entry are public, so the exported types of a declaration file that other files
import like any module (`import type { A } from './api'` reaching `api.d.ts`) were never reported, while knip reports
them.

TypeScript tells two kinds of file apart. A file without a top-level `import`, `export` or `import x = require(…)` is a
script: its declarations are global and apply to every file of the program. A file with one is a module: its exports
are used only through imports. A module can still change the program for every file: `declare global { … }` adds to
the global scope, `declare module 'x' { … }` augments the module `x`, and `export as namespace X` declares a global
for scripts.

Which declaration files are entries?

## Decision Drivers

* An export that no import uses is reported, whatever the extension of its file
* A declaration file that applies without an import keeps what it uses in use
* A published or explicitly listed declaration file is public API

## Considered Options

* Judge a module declaration file like any module
* Keep every declaration file an entry

## Decision Outcome

Chosen option: "Judge a module declaration file like any module", because it follows TypeScript's own split between
scripts and modules, and only a module's exports are used through imports.

* A declaration file is an entry when it applies without an import: a script, or a module file with a top-level
  `declare global`, an `export as namespace` or a `declare module 'x'` augmentation. The parser records whether a file
  is a module and whether it augments the global scope; the module graph parses the declaration files first and makes
  these entries.
* Any other declaration file is a module: an entry only when something else makes it one (the `types`, `typings` or
  `exports` of `package.json`, an `entry` glob, a plugin, or a `tsconfig.json` that lists it in `files`), and otherwise
  in the graph only when a module imports it, where its exports and types are judged.
* A declaration file that cannot be read or parsed stays an entry.
* A `!` pattern of `entry` still leaves declaration files out.
* A declaration file is still never reported by `unused-files`: one beside its runtime file (`Card.d.ts` next to
  `Card.js`) types it without being imported by name.

### Consequences

* Good, because the unused exported types of a module declaration file that only some importers use are reported.
* Good, because what only an unimported module declaration file uses is reported as unused.
* Bad, because a module declaration file that a tool loads by convention without an import or a listing, and that
  augments nothing, now needs an `entry` glob.

### Confirmation

* `crates/skif-js-syntax/tests/fp_graph.rs`:
  `a_declaration_file_declares_without_an_import_unless_it_is_a_plain_module`.
* `crates/skif-js/tests/graph_fixes.rs`: `only_declaration_files_that_apply_without_an_import_are_entries`,
  `unused_types_of_an_imported_module_declaration_file_are_reported`,
  `published_and_listed_module_declaration_files_are_entries` and
  `a_configured_entry_makes_a_module_declaration_file_an_entry`.
* `crates/skif-js-plugins/tests/plugins.rs`: `typescript_declaration_files_listed_in_files_are_entries`.

## Pros and Cons of the Options

### Judge a module declaration file like any module

* Good, because it matches how TypeScript applies declaration files and how knip reports their exports.
* Bad, because whether a declaration file is an entry depends on its parse, not on its name alone.

### Keep every declaration file an entry

* Good, because no declaration file is ever judged, so none can be misjudged.
* Bad, because the unused types of every module declaration file are hidden.
