---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Exportless Declaration Files That Load a Module Are Entries

## Context and Problem Statement

[ADR 0054](0054-module-declaration-files-are-modules.md) makes a declaration file an entry when it applies without an
import: a script, or a module file with `declare global`, `export as namespace` or a `declare module 'x'`
augmentation. Any other declaration file is in the graph only when a module imports it.

Some generated declaration files are modules that export nothing and only load another module. SST writes an
`sst-env.d.ts` into every package of an app:

```ts
/// <reference path="../../sst-env.d.ts" />
import "sst"
export {}
```

`export {}` makes the file a module, so under ADR 0054 nothing makes it an entry, and no module can import it by
name, since it has nothing to give. The package it loads (`sst`) is then reported by `unused-dependencies` in every
package whose code does not import it too. TypeScript still applies such a file wherever it compiles it: its
`import "sst"` and its `/// <reference path>` add the declarations of what they load to the program.

Is a module declaration file that exports nothing and loads a module for its effect an entry?

## Decision Drivers

* A declaration file that applies without an import keeps what it uses in use
* An export that no import uses is reported, whatever the extension of its file

## Considered Options

* Make an exportless module declaration file with a side-effect import an entry
* Keep it a module that only an import brings into the graph

## Decision Outcome

Chosen option: "Make an exportless module declaration file with a side-effect import an entry", because such a file
can do nothing but apply what it loads, as a script does.

* A module declaration file is also an entry when it has no export and at least one side-effect import:
  `import 'x'` or `/// <reference path="…" />`. `ModuleInfo::declares_without_import` holds for it.
* A module declaration file with an export stays a module: its side-effect imports are followed only once a module
  imports it, as before.
* An exportless module declaration file without a side-effect import (only `export {}`, or named imports used by
  local declarations) stays a module: it declares nothing another file can see.

### Consequences

* Good, because a package that only a generated declaration file loads is no longer reported as unused.
* Bad, because such a file keeps what it loads in use even where no `tsconfig.json` compiles it.

### Confirmation

* `crates/skif-js-syntax/tests/fp_graph.rs`:
  `a_declaration_file_declares_without_an_import_unless_it_is_a_plain_module`.
* `crates/skif-js/tests/graph_fixes.rs`: `an_exportless_declaration_file_that_loads_a_module_is_an_entry`.

## Pros and Cons of the Options

### Make an exportless module declaration file with a side-effect import an entry

* Good, because it judges the file by what it can do: an exportless module has no use but its side effects.
* Bad, because the entry depends on the file's imports and exports, not on its name.

### Keep it a module that only an import brings into the graph

* Good, because the entry rule of ADR 0054 stays as written.
* Bad, because no import can ever name the file, so what it loads is never in use.
