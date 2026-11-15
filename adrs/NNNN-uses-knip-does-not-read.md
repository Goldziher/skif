---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Uses knip Does Not Read: Heft Configs, Computed Loads, Type References and Paths knip Skips

## Context and Problem Statement

The knip comparison ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)) sets apart the differences that follow
from how knip reads a project, not from a skif defect ([ADR 0040](0040-more-known-deviations-of-the-knip-comparison.md),
[ADR 0053](0053-plugin-entries-of-nested-roots-and-what-knip-does-not-read.md),
[ADR 0056](0056-knip-tsconfig-declaration-entries-and-module-object-types.md)). The recall of the dependency, file and
export categories still counts many findings only knip reports where knip reads no use that skif reads, and a few
findings only skif reports where knip skips a file or a member check:

* knip has no Heft plugin. A Rush repository's projects load their Heft plugins and rig from `config/heft.json` and
  `config/rig.json`, and a rig's profiles load what the rig lists, which skif's `heft` plugin credits
  ([ADR 0037](0037-heft-and-rig-packages.md)).
* knip follows no `require` whose result a TypeScript `as` asserts a type of (`require('./x') as typeof
  import('./x')`), no `require` or `import()` of a template literal (``require(`./commands/${c}`)``), no default import
  of a CommonJS module read as `module.exports`, no installed `main` below `node_modules` (an Expo app's
  `node_modules/expo/AppEntry.js`, which imports `../../App`), and does not take a namespace spread, put in an object
  literal or passed to a test-runner call for a whole use of the module.
* knip reads no file below a nested `.gitignore`'s `./` pattern (`knip-dot-ignore`), so the dependencies and exports
  only such a file uses are unused for knip only; and it skips every path that contains `node_modules` in a name.
* knip skips the member findings of an enum, or namespace with exported members, that an importer refers to as a type
  (`code: CharCode`) while `nsTypes` is not reported (`getMemberIssues` in its `graph/analyze.js`); skif's type
  reference reads no member.
* knip counts an `import.meta.glob` that reads its files as text (`query: '?raw'`) as a use, skif does not; and knip
  resolves `require('../sum.js')` to `sum.js` where skif, in TypeScript's order, resolves it to `sum.ts` beside it.

Does the harness set these apart, and how does it decide each one from data it reads?

## Decision Drivers

* A deviation is decided by a test the harness runs on data it reads, never by listing findings (ADR 0040)
* The test follows knip's rule as knip implements it, and skif's as skif's parser and plugins read the files
* A difference that skif's analysis should close stays counted

## Considered Options

* Add the deviations below, each tested on the checkout with skif's parser, plugin resolver or oxc
* Keep the findings counted as differences

## Decision Outcome

Chosen option: "Add the deviations below, each tested on the checkout with skif's parser, plugin resolver or oxc",
because each follows from a rule of knip's or skif's that the harness can test.

On knip's side:

* `heft-config`: a dependency only knip reports unused that, in a project where skif's `heft` plugin is enabled, the
  plugin references, or any plugin references from a file of a rig profile (`profiles/<profile>/` of the project or of
  an installed rig), or a peer of such a package. The harness runs skif's plugin resolver for each project that owns
  such a finding, as it already does for plugin entries.
* `type-asserted-require`: an export or namespace export only knip reports in a file a `require` wrapped in `as` loads,
  found in the source text and resolved as the import scan of `imported-by-knip-unused-file` resolves, or that such a
  file re-exports.
* `module-object-values`: an export or namespace export only knip reports in a file that an import uses as a whole
  module object as skif's parser records it, the use `module-object-types` reads on skif's side.
* `commonjs-default-import`: an export or namespace export only knip reports in a CommonJS file without a `default`
  export whose default import a code file reads it through as a property.
* `template-literal-import`: an unused file only knip reports that the glob skif's parser reads from a `require` or
  `import()` of a template literal matches, as skif's graph matches it, or that such a file reaches by relative
  imports through other files only knip reports unused.
* `installed-main`: an unused file only knip reports that the installed `main` of its project imports by relative
  path, or that such a file reaches likewise.
* `knip-dot-ignore` grows a knip side: a dependency that a file a nested `./` pattern hides imports, and an export or
  type such a file imports, by the import scans of `used-in-knip-unused-file` and `imported-by-knip-unused-file`.
* `used-in-knip-unused-file` also takes the peers a used package's installed manifest names, as `bazel-build-file`
  does; `imported-by-knip-unused-file` also takes the enum and namespace members a file only knip reports unused
  reads.

On skif's side:

* `type-referenced-members`: an enum or namespace member only skif reports whose export a code file imports by name
  from the member's file, without an alias, and refers to as a type: a `TSTypeReference` or `TSTypeQuery` of the plain
  identifier, read with oxc, the references knip's visitor records.
* `knip-node-modules-name`: a finding (anything but a manifest finding) in a path that holds `node_modules` inside a
  name and has no `node_modules` directory.
* `raw-glob-read`: an unused file that an `import.meta.glob` reading text matches, or that such a file reaches.
* `typescript-twin`: an unused JavaScript file beside a TypeScript file of the same stem that a code file names by a
  relative specifier with its extension.

### Consequences

* Good, because the recall of the dependency, file, export and member categories counts what skif misses, not what
  knip does not read.
* Bad, because the harness depends on oxc directly for the type references, and parses more files: those holding
  `require(`, a template literal call, `import.meta.glob`, the stem of a CommonJS file or the name of an enum.
* Bad, because several rules credit a whole file (`type-asserted-require`, `module-object-values`) rather than the
  names skif reads through the use, so a knip-only export of such a file that skif credits for another reason is set
  apart too.

### Confirmation

* `tools/bench/src/parity.rs`: the unit tests
  `dependencies_heft_configs_and_rig_profiles_credit_split_off_on_knip_side`,
  `findings_knip_misses_for_files_a_nested_dot_slash_ignore_hides_split_off_on_knip_side`,
  `requires_a_typescript_as_asserts_a_type_of_are_read_from_the_text`,
  `exports_of_a_module_a_type_asserted_require_loads_split_off_on_knip_side`,
  `files_a_template_literal_require_matches_and_what_they_reach_split_off_on_knip_side`,
  `exports_a_default_import_of_a_commonjs_module_reads_split_off_on_knip_side`,
  `files_an_installed_main_imports_and_what_they_reach_split_off_on_knip_side`,
  `value_exports_of_a_module_used_as_a_whole_object_split_off_on_knip_side`,
  `members_of_an_export_an_importer_refers_to_as_a_type_split_off_on_skif_side`,
  `members_a_file_only_knip_lists_unused_reads_split_off_on_knip_side`,
  `peers_of_a_package_a_file_only_knip_lists_unused_imports_are_used_in_that_file`,
  `findings_in_paths_knip_takes_for_node_modules_split_off_on_skif_side`,
  `files_an_import_meta_glob_reads_as_text_split_off_on_skif_side` and
  `javascript_files_named_beside_a_typescript_twin_split_off_on_skif_side` pin each rule and its negative cases.

## Pros and Cons of the Options

### Add the deviations below, each tested on the checkout with skif's parser, plugin resolver or oxc

* Good, because each rule is a test on files and configurations the harness reads.
* Bad, because the rules mirror knip behaviour that may change in a later knip.

### Keep the findings counted as differences

* Good, because nothing new can drift.
* Bad, because the counts overstate what skif must fix, and hide the misses among them.
