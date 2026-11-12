---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Bazel: Static Reading of Starlark, Output Trees That Are Never Read, and Paths Under `bazel run` and `bazel test`

## Context and Problem Statement

In a repository built with Bazel and rules_js, much of what skif looks for lives in Starlark rather than in
`package.json` and tool configs. A package uses a dependency through a label of the `node_modules` that rules_js
links for it, or by loading its `package_json.bzl`; a file is built because a `js_binary` names it as its
`entry_point` or a `ts_project` lists it in `srcs`; generated code exists only in `bazel-bin`; the `bazel-<name>`
convenience link mirrors the whole repository; rules_js and rules_nodejs pin pnpm and Node in `MODULE.bazel`; and a
member lockfile may be live because a `BUILD` file installs from it. skif itself also runs under Bazel: `bazel run`
starts it in a runfiles tree, and `bazel test` allows writes only to a scratch directory.

How does skif read a Bazel repository without running Bazel, and how does it behave when Bazel runs it?

## Decision Drivers

* Starlark is read statically and never evaluated, as tool configs are
  ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md))
* The findings of a Bazel repository do not depend on what was built last
* A file a `BUILD` file builds from is used, but its exports are public only where a caller the analysis cannot see
  may read them
* An import that nothing generates is still `unresolved-imports`
* Outside a Bazel repository nothing changes, and the extra cost is paid only where it changes an answer
* `bazel run //:skif -- --fix` lints and fixes the source tree the user is in, and nothing is written into Bazel's
  output tree
* The environment is read only by the CLI crate ([ADR 0003](0003-library-api-and-embedding.md))

## Considered Options

* Scan `BUILD` and module files as Starlark text for literal labels, paths and one fixed comprehension shape; never
  read output trees; and adapt the CLI's directories to `bazel run` and `bazel test`
* Run `bazel query` or `bazel cquery` for the targets, their sources and outputs
* Evaluate Starlark
* Take every missing relative import in a Bazel repository for generated code

## Decision Outcome

Chosen option: "Scan `BUILD` and module files as Starlark text for literal labels, paths and one fixed comprehension
shape; never read output trees; and adapt the CLI's directories to `bazel run` and `bazel test`", because a text scan
finds what rules_js repositories write without running Bazel, the `BUILD` files say what is generated whether or not
it was built, and the CLI changes are local to the binary.

### The Bazel repository

The Bazel root is the nearest directory with `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` or `WORKSPACE`, searched
no higher than the git repository root. A directory inside one of its output trees has none.

### The `bazel` plugin

* Enabled by a `BUILD.bazel` or `BUILD` file at the top of a package. The `BUILD` files below it are read only when a
  root file of the Bazel repository names `aspect_rules_js`.
* A string literal that is a `node_modules/<name>` label of the main repository, or the `<name>/package_json.bzl` of
  another repository, names `<name>` as a dependency of the label's Bazel package. A dependency named in a package's
  own `BUILD` file that its manifest does not list is unlisted there, reported at the literal's file and line
  ([ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md)).
* `["<template>" % v for v in [<string literals>]]`, with exactly one `%s` in the template, the same loop variable
  and no `if` clause, yields one label per item. Any other comprehension yields nothing; a string followed by `%` or
  `.format` is a template and names nothing; strings inside `glob(...)` are file patterns and name nothing; and a
  label names a dependency only when its name is a valid npm package name.
* The literal `entry_point` of `js_binary`, `js_run_devserver` and `js_test` is an entry, a production entry except
  for `js_test`. The literal `data` of those rules and the `srcs` and `data` of `js_library` and `ts_project` are
  build sources: the module graph starts from them, so they and what they import are used, but their exports are
  judged as any module's ([ADR 0015](0015-plugin-model-and-plugin-set.md)). They are production build sources except
  for `js_test`. A file an entry rule also matches stays an entry. `deps` name targets, not files, and are not read.
* Build sources reach where plugin entries do: one that matches a file of a member nested below the package is a
  build source of that member, and one that names a file of a nested root is handed to that root, where it is an
  entry. `skif explain-usage` reports such a file as `build-source` and starts its import chains there too.
* Nothing is read from `glob()`, `select()`, variables or macros defined in `.bzl` files, and a `BUILD` file in an
  output tree is not read.

### Output trees and generated code

* `bazel-bin`, `bazel-out`, `bazel-testlogs` and `bazel-<workspace>` at the Bazel root are never followed. An import
  that resolves there, directly or through `rootDirs`, is generated code; elsewhere a file reached through `rootDirs`
  is adopted as in any repository ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)).
* A missing relative import is generated when the nearest `BUILD` file at or above its directory declares it in the
  `outs`, `out` or `stdout` of a call (also with another extension or none), or below a directory of its `out_dirs`
  or `out_dir`.

### Convenience links

A workspace member whose first path segment is a symbolic link named `bazel-*` stays in the workspace, as pnpm lists
it, but is excluded from analysis. `nested-workspace-root` reports it once, at its `package.json`, with the detail
`BazelConvenienceLink` and `!bazel-*` as the pattern to add. A real directory named `bazel-*` is an ordinary member.

### Toolchain pins, lockfile consumers and public hoisting

* The toolchain kind `bazel` reads `MODULE.bazel`, `WORKSPACE.bazel` and `WORKSPACE` at the root for a literal
  `pnpm_version` of a call ending in `pnpm` or `npm_translate_lock` and a literal `node_version` of a call ending in
  `toolchain` or `nodejs_register_toolchains`. `toolchain-pin-drift` reads it by default
  ([ADR 0011](0011-lockfile-and-toolchain-rules.md)).
* A member lockfile that a build installs from is not stray. In a Bazel repository its consumers are labels in the
  files Bazel reads for it, labels inside `$(location ...)` and its sibling expansions, and a `glob()` of the
  lockfile's own package that matches it. Docstrings name nothing. Only when those leave a lockfile unnamed is the
  repository listed for its `BUILD` files (skipping hidden directories, `node_modules`, symbolic links and
  `.bazelignore`d directories), read with the Starlark files they load. Under git discovery that listing comes from
  the git index ([ADR 0014](0014-files-the-js-analysis-reads-walk-git-index-fixtures.md)).
* rules_js `public_hoist_packages` dicts in the root module files make their packages available where they are
  hoisted ([ADR 0012](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md)).

### `bazel run` and `bazel test`

* When `BUILD_WORKSPACE_DIRECTORY` is set and not empty, `skif_cli::run_to_exit` changes the current directory,
  before parsing arguments, to `BUILD_WORKING_DIRECTORY` (taken below the workspace if relative), else to the
  workspace directory, so every relative path on the command line means what it means in the user's shell. A
  directory that cannot be entered is an `io` error with exit code 2. `BUILD_WORKING_DIRECTORY` alone changes
  nothing.
* The npm package's `skif` bin does the same with `process.chdir`. `run`, `lint` and `lintAsync` of the addon never
  change directory, so an embedding host stays in step with its own current directory.
* Under `bazel test` (`TEST_TMPDIR` set and not empty), `--cache` without `--cache-location` keeps its store in
  `$TEST_TMPDIR/skif/` ([ADR 0029](0029-performance-caching-and-benchmarks.md)). An explicit `--cache-location`
  wins.

### Consequences

* Good, because the dependencies and files that rules_js targets name are used, and an output the `BUILD` files
  declare is never unresolved.
* Good, because the findings are the same before and after any build, and a build never fills the report with copies
  of root findings under `bazel-<name>/`.
* Good, because the exports of a rules_js repository's sources are judged as knip judges them.
* Good, because a pnpm or Node version Bazel installs that differs from the one outside Bazel is reported, and the
  unsafe fix of `stray-lockfile` never deletes a lockfile a build uses.
* Good, because `bazel run //:skif -- --fix` lints and fixes the source tree, and `bazel test` writes nothing into the
  output tree.
* Bad, because labels, entries and outputs that a macro, a variable or a filtered comprehension declares are not
  found.
* Bad, because an export that only another Bazel target reads through a module name only Bazel maps is reported
  unused.
* Bad, because a Bazel repository with a member lockfile nothing names pays a listing of its directories on every
  run.
* Bad, because the `bazel run` rule is written twice, in Rust and in JavaScript.

### Confirmation

* `crates/skif-js-plugins/src/bazel.rs`: `label_literals_expand_formatted_lists_and_skip_globs_and_templates` and
  `public_hoist_packages_are_the_literal_keys_and_locations`.
* `crates/skif-js-plugins/tests/plugins.rs`: `bazel_build_files_credit_linked_dependencies_and_add_entries`,
  `bazel_build_files_name_nothing_without_rules_js`, `bazel_is_enabled_by_a_build_file_of_the_package` and
  `bazel_format_comprehensions_name_their_packages_and_glob_patterns_none`.
* `crates/skif-js/tests/fp_resolve.rs`: `bazel_outputs_are_generated_whatever_was_built`,
  `build_files_declare_nothing_outside_a_bazel_repository`, `bazel_sources_are_used_and_their_exports_judged` and
  `a_bazel_source_that_is_also_an_entry_keeps_public_exports`.
* `crates/skif-js/tests/explain_absence.rs`: `a_bazel_source_is_a_build_source_whose_exports_are_judged`;
  `crates/skif-js/tests/explain_dependency.rs`:
  `a_bazel_format_comprehension_names_its_packages_and_glob_patterns_name_none`;
  `crates/skif-cli/tests/multi.rs`: `a_bazel_source_of_a_nested_root_is_used_there`.
* `crates/skif-workspace/src/bazel.rs`: `members_inside_a_bazel_convenience_link_are_facts_and_ignored`;
  `crates/skif-rules/src/manifest/nested_workspace_root.rs`:
  `a_member_inside_a_bazel_convenience_link_is_reported_once`.
* `crates/skif-rules/src/toolchain/parse.rs`: `bazel_files_pin_through_rules_js_and_rules_nodejs_calls`;
  `crates/skif-rules/src/toolchain/pin_drift.rs`: `bazel_module_pins_are_checked_against_the_root`.
* `crates/skif-rules/src/manifest/lockfile_consumers.rs`:
  `a_lockfile_only_another_package_names_is_found_by_listing_the_build_files` and
  `a_glob_reaches_only_into_its_own_package`; `crates/skif-rules/src/manifest/stray_lockfile.rs`:
  `under_git_discovery_the_bazel_listing_comes_from_the_git_index`.
* `crates/skif-js/tests/public_hoisting.rs`:
  `bazel_public_hoist_packages_make_packages_available_where_they_are_hoisted`.
* `crates/skif-cli/src/env.rs`: `the_bazel_run_directory_is_the_working_directory_else_the_workspace` and
  `an_empty_test_tmpdir_is_unset`; `crates/skif-cli/tests/cli.rs`:
  `bazel_run_fixes_the_workspace_of_the_working_directory`,
  `bazel_run_resolves_relative_paths_against_the_working_directory` and `a_missing_bazel_run_directory_exits_two`;
  `crates/skif-cli/tests/cache.rs`: `under_bazel_test_the_default_store_is_in_the_test_tmpdir`;
  `scripts/npm-smoke.sh` runs the npm bin under `bazel run`.

## Pros and Cons of the Options

### Scan `BUILD` and module files as Starlark text for literal labels, paths and one fixed comprehension shape; never read output trees; and adapt the CLI's directories to `bazel run` and `bazel test`

* Good, because it needs neither Bazel nor a Starlark interpreter, and costs nothing outside a Bazel repository.
* Bad, because only literal forms and one comprehension shape are understood.

### Run `bazel query` or `bazel cquery` for the targets, their sources and outputs

* Good, because Bazel's own answer covers macros and computed values.
* Bad, because it needs Bazel installed and a configured, fetched repository, takes seconds to minutes, and cannot
  run in the library.

### Evaluate Starlark

* Good, because computed label lists and macros would be read.
* Bad, because it needs an interpreter and the loaded `.bzl` files of external repositories, which a lint run does
  not have.

### Take every missing relative import in a Bazel repository for generated code

* Good, because it needs no reading of `BUILD` files.
* Bad, because a real typo in an import is then never reported.
