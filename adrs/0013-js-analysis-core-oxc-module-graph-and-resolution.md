---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# JS Analysis Core: knip Semantics on oxc, the Module Graph and Resolution

## Context and Problem Statement

knip finds unused files, exports, dependencies and unlisted or unresolved imports in JavaScript and TypeScript
projects. It runs on Node, loads every tool's configuration by executing it, and is slow on large monorepos. skif
reports the same issue types inside one Rust process, next to its manifest rules. That needs a parser, a resolver that
behaves as Node, TypeScript and bundlers do, a module graph, and a way to keep the fast-moving oxc crates from
spreading through the code base. poly, a host that embeds skif, patches the oxc crates to a git copy, so skif and poly
must resolve to the same oxc release.

Which semantics does the JS analysis follow, on which crates, how does it resolve specifiers, which declaration files
are entries, and which package owns a file it reaches?

## Decision Drivers

* knip's results are the reference users compare against; agreement is measured, not assumed
* No Node, no config execution, so the analysis is safe on untrusted pull requests and runs in parallel
* oxc changes break weekly; they must touch as few crates as possible and never the public API
* skif and poly build against one oxc release
* Resolution matches what Node, TypeScript and the bundlers resolve, without guessing; where the compiler and knip
  disagree, the compiler decides
* A declaration file that applies without an import keeps what it uses in use; any other export that no import uses is
  reported, whatever the extension of its file

## Considered Options

* knip semantics implemented natively on oxc, confined to the JS crates, held in lockstep with poly
* Run knip and translate its output
* Port knip's code
* Another parser stack (swc, tree-sitter)

## Decision Outcome

Chosen option: "knip semantics implemented natively on oxc, confined to the JS crates, held in lockstep with poly",
because it removes Node, the binding boundary and config execution, keeps oxc upgrades inside `skif-js-syntax`,
`skif-js-plugins`, `skif-js` and the unpublished `skif-bench`, and is the only option under which poly's patch of the
oxc crates resolves.

### knip as the specification

* knip 6.44.1 (commit `da07b6e9887f9a1eb08545da29755b7f9d309bca`) is the specification: its documentation, issue
  types and observed output define correct behaviour. No knip code is copied; test fixtures may be adapted with
  attribution ([ADR 0033](0033-attribution-and-vendoring.md)). fallow is a design reference only.
* Agreement with knip is a measure, not the goal: where both tools report a file in use, skif fixes its own finding,
  and the comparison harness records the deliberate differences
  ([ADR 0021](0021-knip-and-syncpack-comparison-harness-and-its-known-deviations.md)).
* Each knip issue type maps to the skif rules that report the same findings. The mapping is the data file
  `crates/skif-migrate/src/knip-types.txt`, which migration and `--fix-type` share. knip's `exports` and `types` include
  the namespace rules ([ADR 0018](0018-export-namespace-and-member-rules.md)), and `catalog` and `catalogReferences`
  are the policy rules `unused-catalog-entries` and `unresolved-catalog-references`
  ([ADR 0010](0010-version-semver-range-and-catalog-policies.md)).
* `[js] production` and `--production` are knip's production mode.

### The JS crates

* `skif-js-syntax` is the only crate that names oxc parser types. Per file it produces a skif-owned `ModuleInfo`:
  imports and exports from oxc's module record, plus what a visitor finds (`require()`, `import()` with a static
  argument, `import.meta.glob`, JSDoc tags, enum and namespace members, removal ranges), with in-file references
  resolved by `oxc_semantic`. It also holds the static evaluator of configuration files
  ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)).
* `skif-js-plugins` holds the plugin table and the `package.json` script parser
  ([ADR 0015](0015-plugin-model-and-plugin-set.md)).
* `skif-js` holds the module graph, resolution on `oxc_resolver`, the JS rules as a `RuleProvider`, and the parallel
  worklist that builds the graph from entries.
* The unpublished `skif-bench` also parses with oxc, to mirror knip's reading of a module in the comparison harness
  ([ADR 0021](0021-knip-and-syncpack-comparison-harness-and-its-known-deviations.md)). No other crate depends on oxc,
  and `scripts/check-dependency-tree.sh` enforces it
  ([ADR 0002](0002-crates-publishing-and-repository-consistency-checks.md)).
* No oxc type appears in a public item of any skif crate ([ADR 0003](0003-library-api-and-embedding.md)); review holds
  the boundary. The TypeScript syntax skif understands is the syntax the pinned parser understands.

### oxc lockstep

* `oxc_allocator`, `oxc_ast`, `oxc_ast_visit`, `oxc_parser`, `oxc_semantic`, `oxc_span` and `oxc_str` are caret
  requirements on the oxc release that poly pins by revision. `oxc_resolver` is a separate project with its own caret
  requirement and is not part of the lockstep.
* An oxc bump is two coordinated pull requests, in skif and in poly, merged skif first; neither moves alone.
* The `oxc lockstep` workflow runs `scripts/check-oxc-git-patch.sh` weekly and on changes to `Cargo.toml` or
  `Cargo.lock`: it patches the lockstep crates to the release tag's git commit through `--config` flags, builds skif
  with `js-analysis`, and fails on an unused patch, a duplicate oxc crate or a crate that does not resolve to the git
  copy. `mise run oxc:lockstep` runs it locally, optionally against the next tag.

### Resolution

* A builtin is a `node:` specifier, an exact builtin name, or a builtin subpath (`fs/promises`); `buffer/` and
  `events/` name the npm packages.
* A symbolic link to a file the analysis also finds is that file. A link out of the workspace, or to a file outside the
  analysis, stays a module of its own.
* Every literal leaf of `package.json#imports` is reachable; a pattern leaf is reached only by the specifiers that use
  it.
* A template literal that starts with a relative path is a glob of the files it can name, with `*` per expression, and
  each match counts as read in full; any other computed specifier is an unknown target.
* A specifier in a style block of a `.vue`, `.svelte` or `.astro` file that the script resolver cannot find is looked
  up as a stylesheet. Stylesheet specifiers resolve aliases, `paths`, `baseUrl`, `resolve.modules` and Sass partials
  with the stylesheet resolver.
* A bundler's `resolve.extensions` apply to the files of the innermost package that declares them, not to the
  workspace.
* The `paths`, output directories and conditions of a `tsconfig` reached through `references` in another workspace
  package do not apply to a package that lists that package as a dependency; a package that does not list it still
  resolves through them.
* Picomatch extglob groups in plugin and configuration globs become brace sets: `(a|b)`, `@(a|b)` and `+(a|b)` become
  `{a,b}`, `?(a|b)` and `*(a|b)` become `{a,b,}`, and `!(a|b)` becomes `*`. The approximations err towards more entries,
  never fewer. A group without `|` stays literal, so a route group such as `app/(group)` names a directory.
* Every resolver set of a run (scripts, per-package bundler extensions, stylesheets) is built on one `oxc_resolver`
  cache, so a path is looked up once.

### Resolution order

skif follows TypeScript's order where a `paths` mapping starts with the name of a package the project lists:

* The `paths` of `[js.workspaces.<dir>.paths]` are tried before lookup directories (`baseUrl`, `resolve.modules`),
  installed packages and workspace members, for scripts and stylesheets alike, innermost package first. They also come
  before the `paths` of the importer's `tsconfig.json` or `jsconfig.json`, which come before installed packages inside
  the resolver.
* A pattern whose targets all miss, generated code that is not there yet included, falls through to the package or
  member lookup, as TypeScript does. Only when that finds nothing is the specifier judged as generated, unresolved or
  the package it names ([ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md)).
* A mapping that leads into a workspace member's directory or into `node_modules` is an import of that package. A
  mapping that leads into the importer's own sources is a local import, which uses no listed dependency; a mapping to a
  declaration file only types the import, which still uses the listed package
  ([ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md)).

### Declaration files

Declaration files (`.d.ts`, `.d.mts`, `.d.cts`) follow TypeScript's split between scripts and modules. A file without a
top-level `import`, `export` or `import x = require(…)` is a script and applies to the whole program; any other file is
a module.

* A declaration file is an entry when it applies without an import:
  * a script;
  * a module with a top-level `declare global`, `export as namespace` or `declare module 'x'` augmentation;
  * a module with no export and at least one side-effect import (`import 'x'` or `/// <reference path="…" />`).

  The parser records these facts (`ModuleInfo::declares_without_import`), and the module graph parses the declaration
  files first and makes these entries.
* Any other declaration file is a module. It is an entry only when something else makes it one: the `types`, `typings`
  or `exports` of `package.json`, an `entry` glob, a plugin, or a `tsconfig.json` that lists it in `files`. Otherwise it
  is in the graph only when a module imports it, and then its exports and types are judged.
* A declaration file that cannot be read or parsed stays an entry. A `!` pattern of `entry` still leaves declaration
  files out.
* A declaration file is never reported by `unused-files`: one beside its runtime file (`Card.d.ts` next to `Card.js`)
  types it without being imported by name.

Where the resolution order or the declaration-file split makes skif and knip disagree, the difference is a known
deviation under the general rule of [ADR 0021](0021-knip-and-syncpack-comparison-harness-and-its-known-deviations.md).

### Ownership

* A file belongs to the workspace package whose directory holds it. A file of the root (in no package's directory and
  not a listed source file) that a package reaches first through `rootDirs` is adopted by that package, and passes the
  owner on to the root files it reaches by relative imports below the same root directory. Files in Bazel output trees
  follow [ADR 0031](0031-bazel.md).
* A module keeps the owner it got when first reached. The graph is built in rounds from entries in path order and
  merged in a fixed order, so the owner is the same in every run.
* An adopted file matches none of its owner's `ignore` or project globs, since it lies outside the owner's directory;
  the root's `ignore` globs still apply.

### Not built

A config helper imported from another workspace package credits nothing in that package: crediting it wholesale would
hide real findings, and crediting only what the helper names needs the helper evaluated. The ignored tests in
`crates/skif-js/tests/monorepo_regressions.rs` specify the gap. The documentation claims no speed-up over knip that is
not a published, dated measurement ([ADR 0029](0029-performance-caching-benchmarks-and-budgets.md)).

### Consequences

* Good, because parsing, resolution and analysis run in one process on rayon, with the parser and resolver knip itself
  uses, so syntax coverage and resolution start aligned.
* Good, because running skif on an untrusted pull request reads files and executes nothing.
* Good, because an oxc upgrade touches only the JS crates, and drift from poly fails skif's scheduled CI first.
* Good, because the imports of generated code reached through `rootDirs` are judged against the package that imports
  it.
* Good, because `[js.workspaces.<dir>.paths]` and `tsconfig.json` `paths` resolve in the compiler's order, so a local
  file a mapping reaches is not reported unused and a dependency a mapping only shadows is.
* Good, because the unused exported types of a module declaration file are reported, while a file that applies without
  an import keeps what it loads in use.
* Bad, because knip's edge cases are rediscovered through the comparison, not inherited, and the comparison sets apart
  the findings where skif follows the compiler instead.
* Bad, because skif's oxc upgrades follow poly's cadence, and the lockstep check needs the network and builds oxc from
  source.
* Bad, because the extglob and computed-specifier approximations keep files alive that no runtime value reaches.
* Bad, because a module declaration file that a tool loads by convention without an import or a listing, and that
  augments nothing, needs an `entry` glob, and an exportless one that loads a module keeps it in use even where no
  `tsconfig.json` compiles it.
* Bad, because a generated file reached first from one package stays with it when another package reaches it later,
  and a package that wraps its configuration in a shared helper can be reported for what only the helper names.

### Confirmation

* `scripts/check-oxc-git-patch.sh`, run by `.github/workflows/oxc-lockstep.yml` and `mise run oxc:lockstep`, and
  `scripts/check-pins.sh`, which fails when an oxc requirement is not a caret range or the lockstep crates name more
  than one release.
* `crates/skif-js/tests/fp_resolve.rs`: `files_reached_through_root_dirs_belong_to_the_importing_package`,
  `root_dirs_into_another_workspace_package_leave_its_files_with_it`,
  `relative_imports_out_of_the_root_dir_of_an_adopted_file_are_not_adopted` and
  `the_first_package_to_reach_a_shared_generated_file_owns_it`; `crates/skif-js/tests/js_config.rs`:
  `files_adopted_through_root_dirs_keep_the_ignore_globs_of_the_root`.
* `crates/skif-js/src/resolve.rs`: `relative_imports_cross_root_dirs` and
  `another_root_dir_names_the_root_a_resolved_file_lies_below`.
* `crates/skif-js/tests/paths_before_packages.rs`:
  `tsconfig_paths_into_own_sources_do_not_use_the_member_named_like_the_prefix`,
  `tsconfig_paths_into_own_sources_do_not_use_the_installed_package_named_like_the_prefix`,
  `configured_paths_win_over_the_member_named_like_the_prefix`,
  `configured_paths_win_over_the_installed_package_named_like_the_prefix`,
  `configured_paths_whose_target_is_missing_fall_through_to_the_member`,
  `paths_whose_target_is_missing_fall_through_to_the_installed_package`,
  `an_import_no_paths_entry_matches_still_uses_the_listed_package`,
  `paths_to_a_declaration_file_still_use_the_listed_package` and
  `configured_paths_win_over_the_paths_of_the_compiler_config`; `crates/skif-js/tests/fp_graph.rs`:
  `a_listed_package_a_path_alias_maps_into_own_sources_is_unused`.
* `crates/skif-js-syntax/tests/fp_graph.rs`:
  `a_declaration_file_declares_without_an_import_unless_it_is_a_plain_module`.
* `crates/skif-js/tests/graph_fixes.rs`: `only_declaration_files_that_apply_without_an_import_are_entries`,
  `unused_types_of_an_imported_module_declaration_file_are_reported`,
  `published_and_listed_module_declaration_files_are_entries`,
  `a_configured_entry_makes_a_module_declaration_file_an_entry` and
  `an_exportless_declaration_file_that_loads_a_module_is_an_entry`.
* `crates/skif-js-plugins/tests/plugins.rs`: `typescript_declaration_files_listed_in_files_are_entries`.
* `crates/skif-js/tests/monorepo_regressions.rs` holds the ignored tests that specify the config-helper gap.

## Pros and Cons of the Options

### knip semantics implemented natively on oxc, confined to the JS crates, held in lockstep with poly

* Good, because it is fast, safe and confines oxc to the JS crates and the benchmark harness.
* Good, because resolution and declaration-file entries follow the compiler, so findings match what the build uses.
* Bad, because every knip behaviour is re-implemented, and oxc upgrades are coordinated across two repositories.

### Run knip and translate its output

* Good, because results would match knip exactly.
* Bad, because it needs Node, executes configuration, and keeps knip's speed.

### Port knip's code

* Good, because edge cases would be inherited.
* Bad, because a line-by-line port carries knip's design into a different language and its licence obligations into
  every file.

### Another parser stack (swc, tree-sitter)

* Good, because swc is mature and tree-sitter is error-tolerant.
* Bad, because neither shares knip's resolver, and a second parser stack would not match poly's.
