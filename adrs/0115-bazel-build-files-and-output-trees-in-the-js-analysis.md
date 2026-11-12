---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0062", "0110"]
---

# Bazel BUILD Files and Output Trees in the JS Analysis

## Context and Problem Statement

In a repository built with Bazel and rules_js, much of what the JS analysis looks for lives in Starlark rather than in
`package.json` and tool configs. A package uses a dependency by a label of the `node_modules` that
`npm_link_all_packages` links for it (`":node_modules/esbuild"`) or by loading its `package_json.bzl`
(`"@npm//packages/app:esbuild/package_json.bzl"`), and a file is built because a `js_binary` names it as its
`entry_point` or a `ts_project` lists it in `srcs`. skif read none of this, so such dependencies were
`unused-dependencies` and such files `unused-files` (GitHub issue #169).

Generated code is a second gap. A `js_run_binary` writes `src/gen/schema.js` only to `bazel-bin/packages/app`, which a
source file imports as `./gen/schema.js`, often with `"rootDirs": [".", "../../bazel-bin/packages/app"]`. Without
`rootDirs` the import was `unresolved-imports`. With it, the import was suppressed only while `bazel-bin` did not
exist, reported again after a build that did not build that target, and, when the file existed, the copy was read
and its imports judged ([ADR 0110](0110-files-reached-through-rootdirs-belong-to-the-importing-package.md)). The
findings depended on what was built last and for which configuration, as `bazel-bin` moves between `-fastbuild` and
`-opt` (GitHub issue #170).

How should the JS analysis read a Bazel repository's `BUILD` files, and treat its output trees?

## Decision Drivers

* Starlark is read statically and never evaluated, as tool configs are ([ADR 0010](0010-js-analysis-on-oxc.md))
* The findings of a Bazel repository do not depend on what was built last
* An import that nothing generates is still `unresolved-imports`
* Outside a Bazel repository nothing changes
* Library crates do no I/O beyond the `FileSystem` trait

## Considered Options

* A `bazel` plugin that scans `BUILD` files, and output trees that are never read, with `BUILD` files declaring the
  generated files
* A `bazel` plugin, and every missing relative import in a Bazel repository taken for generated
* Run `bazel query` or `bazel cquery` for the targets, their sources and outputs

## Decision Outcome

Chosen option: "A `bazel` plugin that scans `BUILD` files, and output trees that are never read, with `BUILD` files
declaring the generated files", because a text scan finds the labels and literal paths that rules_js repositories
write, without running Bazel, and the `BUILD` files say what is generated whether or not it was built.

* The Bazel repository is the nearest directory at or above a directory holding `MODULE.bazel`, `REPO.bazel`,
  `WORKSPACE.bazel` or `WORKSPACE`, looked for up to the git repository root, as `stray-lockfile` looks for it
  ([ADR 0107](0107-stray-lockfiles-a-build-installs-from-and-a-dependency-in-none-of-several-catalogs.md)). There is
  none for a directory inside one of its output trees (a test's runfiles below `bazel-out`).
* The `bazel` plugin is enabled by a `BUILD.bazel` or `BUILD` file at the top of a package, and reads the `BUILD`
  files below it only when a root file of the Bazel repository names `aspect_rules_js`. The plugin row has no knip
  counterpart; the table holds 206 plugins.
* A string literal that is a `node_modules/<name>` label of the main repository (`":node_modules/x"`,
  `"//pkg:node_modules/@scope/x"`, `"@//pkg:node_modules/x/dir"`), or the `<name>/package_json.bzl` of another
  repository (`"@npm//pkg:x/package_json.bzl"`), names `<name>` as a dependency of the package whose directory is
  the label's Bazel package. A package uses the dependencies so named for it, by any `BUILD` file below it, nested
  packages' files included; a label of another package's `node_modules` is that package's. A dependency named in the
  package's own `BUILD` file that its manifest does not list is unlisted at that file, as for a tool config
  ([ADR 0086](0086-packages-tool-configs-name-are-unlisted-at-the-config.md)).
* The keyword arguments of the top-level calls give entries: the `entry_point` and `data` of `js_binary`,
  `js_run_devserver` and `js_test`, and the `srcs` and `data` of `js_library` and `ts_project`, when a literal names
  an existing file of the package, by path or as `":path"`. They are production entries, except those of `js_test`.
  A value inside a nested call (`glob([...])`, `select({...})`) and a variable name nothing, nor does a macro defined in
  a `.bzl` file. A nested package's `BUILD` file gives no entries, and a `BUILD` file in an output tree is not read.
* In a Bazel repository a file below one of the `bazel-*` entries at its top (`bazel-bin`, `bazel-out`,
  `bazel-testlogs`, `bazel-<workspace>`) is never followed: an import that resolves there, directly or through
  `rootDirs`, is generated code. This amends ADR 0110 for Bazel repositories; elsewhere a file reached through
  `rootDirs` is adopted as before.
* A relative import of a missing file is generated when the nearest `BUILD.bazel` or `BUILD` file at or above the
  file's directory, up to the Bazel root, declares it: a literal of the `outs`, `out` or `stdout` of a call there, also
  with another extension or none, or a directory of its `out_dirs` or `out_dir` above it. A `rootDirs` directory in
  an output tree does not make a missing import generated, whether it exists or not.

### Consequences

* Good, because the dependencies and files that rules_js targets name are used, and an output the `BUILD` files
  declare is never unresolved.
* Good, because the findings are the same before and after any build: nothing is read from an output tree.
* Good, because an import of a file that no target declares is still `unresolved-imports`.
* Neutral, because a generated file's own imports are no longer judged; the dependencies its target names
  (`srcs = [":node_modules/zod"]`) are used instead.
* Bad, because outputs and entries that a macro of a `.bzl` file or a computed value declares are not found: such an
  import is unresolved and such a file can be reported unused.
* Bad, because a label of a package's `node_modules` written in a sibling package's `BUILD` file is not read for it, as
  each package reads only the `BUILD` files below its own directory.

### Confirmation

* `crates/skif-js-plugins/src/bazel.rs`: `literals_skip_comments_and_read_every_quote_style`,
  `calls_keep_the_literal_values_of_their_keyword_arguments`,
  `comparisons_and_augmented_assignments_are_no_keyword_arguments`,
  `declared_outputs_match_files_by_stem_and_files_below_directories`, `package_paths_are_files_of_the_package`,
  `the_repository_root_is_the_nearest_directory_with_a_root_file_inside_git` and
  `output_paths_lie_below_a_bazel_link_at_the_root`.
* `crates/skif-js-plugins/src/plugins/bazel.rs`: `labels_of_linked_packages_name_the_dependency_and_its_bazel_package`.
* `crates/skif-js-plugins/tests/plugins.rs`: `bazel_build_files_credit_linked_dependencies_and_add_entries`,
  `bazel_build_files_name_nothing_without_rules_js`, `bazel_is_enabled_by_a_build_file_of_the_package` and
  `bazel_root_labels_from_nested_packages_count_and_output_trees_do_not`; `ADR_IDS` lists the 206 ids.
* `crates/skif-js/tests/fp_resolve.rs`: `bazel_outputs_are_generated_whatever_was_built` (no `rootDirs`, `rootDirs`
  without `bazel-bin`, with the output built, and after a build of another target give the same findings) and
  `build_files_declare_nothing_outside_a_bazel_repository`.
* `scripts/check-plugin-list.sh` passes.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A `bazel` plugin that scans `BUILD` files, and output trees that are never read, with `BUILD` files declaring the generated files

* Good, because it needs no Bazel installation and reads only the files of the packages analysed.
* Good, because a typo in a relative import is still reported.
* Bad, because only literal labels and paths count.

### A `bazel` plugin, and every missing relative import in a Bazel repository taken for generated

* Good, because no output declaration needs to be read.
* Bad, because no missing relative import of a Bazel repository would ever be reported.

### Run `bazel query` or `bazel cquery` for the targets, their sources and outputs

* Good, because macros and computed values are evaluated.
* Bad, because it runs an external tool, which the analysis never does, and is slow on a cold server.

## More Information

* Amends: [ADR 0062](0062-custom-element-registrations-and-the-taskr-plugin.md) (the plugin count) and
  [ADR 0110](0110-files-reached-through-rootdirs-belong-to-the-importing-package.md) (files reached in a Bazel output
  tree)
* Issues: [#169](https://github.com/Goldziher/skif/issues/169), [#170](https://github.com/Goldziher/skif/issues/170)
* rules_js: <https://github.com/aspect-build/rules_js>
* See the [ADR index](index.md).
