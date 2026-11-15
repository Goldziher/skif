---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# knip's tsconfig Declaration Entries and Its Opaque Module Objects

## Context and Problem Statement

The knip comparison ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)) sets apart the differences that follow
from how knip reads a project, not from a skif defect ([ADR 0040](0040-more-known-deviations-of-the-knip-comparison.md),
[ADR 0051](0051-members-whose-users-skif-does-not-read.md),
[ADR 0053](0053-plugin-entries-of-nested-roots-and-what-knip-does-not-read.md)). Two kinds of difference remain:

* knip makes every declaration file (`.d.ts`, `.d.mts`, `.d.cts`) that the `tsconfig.json` of one of its workspaces
  compiles a production entry: the paths `files` lists and the files its `include` globs select (everything below the
  configuration's directory when it sets neither), less its `exclude` globs, `node_modules` and the `outDir`. The
  exports of an entry are public and its imports are read. skif judges a module declaration file like any module, an
  entry only when something else makes it one
  ([ADR 0054](0054-module-declaration-files-are-modules.md),
  [ADR 0055](0055-exportless-declaration-files-that-load-a-module.md)). So the unused types of such a file are skif's
  only, the unlisted dependencies and unresolved imports of one that nothing imports are knip's only, and a file that
  only such a declaration file imports (angular's `*_isolated.golden.d.ts` compliance files) is unused for skif only.
* knip records `import()` of a module, and a namespace it cannot follow by member, as an opaque use that credits every
  export of the module, types included. skif's whole use of a module object counts its value exports only
  ([ADR 0018](0018-export-namespace-and-member-rules.md)), so the types of a module that an `import()` map loads (a
  generated `crm.apps.generated.ts` of `import("./<app>/lib/CrmService")` entries) are skif's only.

Does the harness set these apart, and how does it decide each one from data it reads?

## Decision Drivers

* A deviation is decided by a test the harness runs on data it reads, never by listing findings (ADR 0040)
* The test follows knip's rule as knip implements it, and skif's as skif's parser reads the file
* A difference that skif's analysis should close stays counted

## Considered Options

* Add `tsconfig-declaration-file` and `module-object-types`
* Keep the two kinds counted as differences

## Decision Outcome

Chosen option: "Add `tsconfig-declaration-file` and `module-object-types`", because each follows from a rule of knip's
that the harness can test.

* `tsconfig-declaration-file`: the harness reads the `tsconfig.json` of each of knip's workspaces (the root and the
  members with a manifest) as knip's `loadTSConfig` does: `files`, `include` and `exclude` through `extends`, each
  relative to the configuration that sets it, an entry without a wildcard or extension standing for its directory, and
  hidden directories skipped as knip's globs skip them. It sets apart
  * an export, type, or enum or namespace member only skif reports in such a declaration file;
  * an unlisted dependency or unresolved import only knip reports in one that no code file of the checkout imports:
    the import scan of `imported-by-knip-unused-file` resolves the imports, a source file it resolves to taking the
    import over a declaration file beside it, and a specifier it does not resolve imports a declaration file when it
    names the file, its stem with or without a built extension, or the directory of an `index` declaration file;
  * an unused file only skif reports that such a declaration file reaches by relative imports, directly or through
    other files only skif reports unused.
* `module-object-types`: a type only skif reports (`types`, `nsTypes`) in a file that an import in a code file of the
  checkout uses as a whole module object, as skif's parser records it (`import()`, `require`, a namespace used as a
  whole), directly or through the `export … from` and `export *` re-exports of the files in between. Only the files
  that hold `import(`, `require(` or `* as ` are parsed.

### Consequences

* Good, because the differences left in these files are the ones skif needs a fix for.
* Bad, because the harness reads every workspace's `tsconfig.json` and parses the files that may use a module object
  as a whole, which adds to each repository's classification time.
* Bad, because a declaration file that knip compiles through a `tsconfig.json` option the harness does not read (a
  `references` project, a `--tsConfig` other than `tsconfig.json`) stays a difference.

### Confirmation

* `tools/bench/src/parity.rs`: the unit tests
  `the_declaration_files_a_workspace_tsconfig_compiles_are_the_ones_knip_makes_entries`,
  `declaration_files_no_code_file_imports_are_told_apart_from_imported_ones`,
  `findings_that_knip_s_tsconfig_declaration_entries_explain_split_off` and
  `types_of_a_module_an_import_uses_as_a_whole_object_split_off_on_skif_side` pin the two rules and their negative
  cases.

## Pros and Cons of the Options

### Add `tsconfig-declaration-file` and `module-object-types`

* Good, because each rule is a test on configurations and files the harness reads.
* Bad, because both mirror knip behaviour that may change in a later knip.

### Keep the two kinds counted as differences

* Good, because nothing new can drift.
* Bad, because the count overstates what skif must fix.
