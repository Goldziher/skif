---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# How knip Reads a Module: Opaque Imports, Spied Namespaces, Namespace Re-exports and Entries It Makes

## Context and Problem Statement

The knip comparison ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)) sets apart the differences that follow
from how knip reads a project, not from a skif defect ([ADR 0040](0040-more-known-deviations-of-the-knip-comparison.md),
[ADR 0056](0056-knip-tsconfig-declaration-entries-and-module-object-types.md),
[ADR 0058](0058-uses-knip-does-not-read.md)). The export and type categories still count findings on both sides whose
cause is in how knip's visitors and graph read a module:

* knip binds an `import()` to names only in a few shapes (`handleVariableDeclarator`, `handleImportThen`, a loader
  callback, an awaited `Promise.all` given to an array pattern) and reads every other `import()` with a literal
  specifier as opaque, crediting every export, types included. An `import()` it binds to one name is an import of
  `default`. Its Jest plugin reads `jest.requireActual` as opaque too, and its Vitest plugin reads
  `vi.mock(import('./x'), () => ({ … }))` for side effects.
* With `nsExports` and `nsTypes` off, knip skips the exports of a module whose namespace imports are all referenced as
  a whole (`hasStrictlyNsReferences` in its `graph/analyze.js`). A spy call (`vi.spyOn(ns, 'a')`) is such a reference
  for knip and a use of `a` for skif. Its `ArrayExpression` visitor and `Object.keys` branch mark a namespace opaque by
  name, without checking for a parameter that shadows it (`['ls-tree', path]` beside `import * as path`).
* knip records `export * as X from` as a re-export of the source module and never lists `X` among the file's exports.
  It reads `CommonJS` exports in JavaScript files only (`isJS`), and records a qualified import type
  (`import('./x').Name`) as a side-effect import of the module, with no name.
* knip's `node` plugin makes test-like files entries when a script runs `node --test`; skif has no such entries. skif's
  Bazel plugin makes the `index.ts` of each entry point an `ng_package` names an entry; knip has no Bazel plugin. A
  `.d.ts` entry's re-exports, and the re-exports of a declaration or JavaScript file a manifest publishes as it is,
  are public in the tool that makes the file an entry.

Does the harness set these apart, and how does it decide each one from data it reads?

## Decision Drivers

* A deviation is decided by a test the harness runs on data it reads, never by listing findings (ADR 0040)
* The test follows knip's rule as knip 6 implements it, and skif's as skif's parser and plugins read the files
* A difference that skif's analysis should close stays counted

## Considered Options

* Add the deviations below, each tested on the checkout with skif's parser and oxc
* Keep the findings counted as differences

## Decision Outcome

Chosen option: "Add the deviations below, each tested on the checkout with skif's parser and oxc", because each
follows from a rule of knip's or skif's that the harness can test. The harness reads the shapes knip's visitors match
with oxc, as knip reads its own oxc tree (parentheses kept), and the imports with skif's parser. Every credit is
followed through the `export … from` and `export *` re-exports of the files in between. Only the files holding
`import(`, `requireActual` or `* as ` are parsed for the reads.

On skif's side:

* `opaque-dynamic-import`: an export or type in a file an `import()` that knip reads as opaque loads.
* `dynamic-import-default`: a `default` export of a file an `import()` that knip binds to one name loads.
* `jest-require-actual`: an export or type of a file `jest.requireActual` loads (also through the `jest` of
  `@jest/globals`).
* `spied-namespace`: an export or type of a file whose namespace imports only spy calls read, no namespace import of
  the file being read through a member anywhere in the checkout.
* `enumerated-namespace`: an export or type of a file whose namespace import's name an array literal holds or
  `Object.keys` (`values`, `entries`, `getOwnPropertyNames`) takes.
* `namespace-re-export`: an export or namespace export its file declares as `export * as X from`, of any file.
* `typescript-commonjs-export`: an export or namespace export that a `CommonJS` assignment of a file that is no
  JavaScript file exports, as skif's parser reads such assignments.
* `node-test-entry`: an export or type in a file that matches the `node` plugin's patterns, in every workspace when a
  root script runs `node --test` (or `nub --test`), else in a workspace whose own script does.
* `tsconfig-declaration-file` grows: an export or type that a tsconfig declaration entry re-exports.

On knip's side:

* `ng-package-entry`: an export or type in, or re-exported by, an entry point an `ng_package` of a `rules_js`
  repository names: the `index.ts` of each Bazel package its `deps` labels name inside the `BUILD` file's npm package,
  as skif's Bazel plugin reads them.
* `import-type-qualifier`: an export or type a qualified import type names (`import('./x').A.B` names `A`).
* `published-source` grows: an export or type that a declaration or JavaScript file the manifest publishes as it is
  re-exports, and the harness resolves a workspace package's subpath into unbuilt output to its source, as skif does
  (`next/dist/server/web/types` names `src/server/web/types.ts`).
* `type-asserted-require` takes a template literal specifier without a substitution, and `module-object-values` a
  named import of a namespace re-export that escapes (`import { listSearch } from './methods'` and
  `methods = { listSearch }`, where `methods/index.ts` has `export * as listSearch from './listSearch'`).

Not modelled, and still counted: a `vi.importActual`, which knip does not read either; knip's loader import with no
`default`, read as a use of every export; next.js's `hot-reloader-pages` default and `devlow-bench` constants; and
vscode's `detect.ts`, `gulpfile.editor.ts`, `nativeBuildEnv.ts` and `semver.js`.

### Consequences

* Good, because the export and type recall counts what skif misses, not what knip reads otherwise.
* Bad, because the harness parses more files with oxc and mirrors more of knip's visitor shapes, which a later knip may
  change.
* Bad, because most rules credit a whole file, so a one-sided export of such a file that differs for another reason is
  set apart too.

### Confirmation

* `tools/bench/src/parity.rs`: the unit tests
  `import_expressions_knip_binds_and_those_it_reads_as_opaque_are_told_apart`,
  `exports_of_a_module_an_opaque_or_bound_import_expression_loads_split_off_on_skif_side`,
  `exports_of_a_module_jest_require_actual_loads_split_off_on_skif_side`,
  `exports_of_a_module_whose_namespace_imports_only_spy_calls_read_split_off_on_skif_side`,
  `exports_of_a_module_whose_namespace_name_an_array_or_object_keys_holds_split_off_on_skif_side`,
  `types_a_qualified_import_type_names_split_off_on_knip_side`,
  `namespace_re_exports_and_commonjs_exports_of_typescript_files_split_off_on_skif_side`,
  `files_knip_s_node_plugin_matches_are_read_as_its_patterns_read_them`,
  `exports_of_files_knip_s_node_plugin_makes_entries_split_off_on_skif_side`,
  `exports_a_tsconfig_declaration_entry_re_exports_split_off_on_skif_side`,
  `bazel_labels_name_the_package_skif_s_bazel_plugin_reads`,
  `exports_an_ng_package_entry_point_re_exports_split_off_on_knip_side`,
  `a_workspace_subpath_into_unbuilt_output_resolves_to_its_source`,
  `exports_a_published_declaration_file_re_exports_split_off_on_knip_side`,
  `value_exports_a_named_import_of_a_namespace_re_export_uses_as_a_whole_split_off_on_knip_side` and
  `requires_a_typescript_as_asserts_a_type_of_are_read_from_the_text` pin each rule and its negative cases.

## Pros and Cons of the Options

### Add the deviations below, each tested on the checkout with skif's parser and oxc

* Good, because each rule is a test on files and configurations the harness reads.
* Bad, because the rules mirror knip behaviour that may change in a later knip.

### Keep the findings counted as differences

* Good, because nothing new can drift.
* Bad, because the counts overstate what skif must fix, and hide the misses among them.
