---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Bazel Test Globs and Angular Packages, and Relative Imports Into Unbuilt Build Output

## Context and Problem Statement

`unused-class-members` credits a member only where a used file reads it, so a file the analysis does not reach hides
every read it holds. On large repositories most such files are reached by the build, not by an import:

* A Bazel repository builds its test files from a `testonly` `ts_project` whose `srcs` is a `glob`
  (`glob(["**/*_spec.ts"])`), and a `jasmine_test` runs that target. ADR 0031 reads no `glob`, so every spec file is
  `unused-files`, and so is every helper only specs import.
* rules_angular's `ng_package` publishes one npm package with an entry point for each Bazel package its `deps` name
  (`//packages/common/http`), each built from that package's `index.ts`. Nothing imports a secondary entry point
  inside the repository, so its `index.ts` and the `public_api.ts` it re-exports are `unused-files`, and the exports
  of the library are judged as if it had no public API. rules_angular's `ng_project`, its `ts_project`, is not read.
* A package publishes small files beside its build output that load it (`client.js` holding
  `module.exports = require('./dist/client/index')`). ADR 0035 maps a published path or a plugin entry in build output
  back to its source, but a relative import inside the package that names its unbuilt output is a missing file, taken
  for generated code when `.gitignore` lists `dist`, and the source behind it is never reached.

How do these count, without evaluating Starlark and without making every file of a `glob` used?

## Decision Drivers

* Starlark is read statically and never evaluated ([ADR 0031](0031-bazel.md))
* A test file is an entry because a runner finds it by pattern; the plugins of every test runner give such patterns
* A dead production file stays `unused-files`, also when a `glob` of a production target matches it
* The public API of a package is what it publishes, whatever imports it inside the repository
* A file a build compiles to stands for the same source wherever it is named
  ([ADR 0035](0035-plugin-entries-in-build-output.md))

## Considered Options

* Read the include patterns of the `glob` of test targets only
* Read every `glob` of every rule, with its `exclude` patterns
* Keep ADR 0031 and leave these files to an `entry` setting

## Decision Outcome

Chosen option: "Read the include patterns of the `glob` of test targets only", because a test target's `glob` plays
the part a test runner's file pattern plays in any other repository, while a production target's `glob` matches
every file of a directory, dead or not, and would hide the files `unused-files` exists to find. `ng_package` entry
points and relative imports into unbuilt output are read with it. This changes in part what
[ADR 0031](0031-bazel.md) reads.

### Test targets

* A call of a rule ADR 0031 reads (`js_binary`, `js_run_devserver`, `js_test`, `js_library`, `ts_project`) or of
  `ng_project` with the literal `testonly = True` is a test target, as `js_test` is: its literal files count only
  outside `--production`.
* The `srcs` and `data` of a test target also name the include patterns of a `glob(...)` (also `native.glob`) that
  is their value or is added to it (`[":a.ts"] + glob([...])`): the string literals of the list given first or as
  `include`, relative to the `BUILD` file. They are build sources outside `--production`: the files they match are
  used and their imports followed, but their exports are judged.
* The `exclude` patterns of such a `glob` are not applied, and a `glob` inside another call (`select({...})`) or whose
  patterns are a variable names nothing. A `glob` of a target that is no test still names nothing.
* `ng_project` is read as `ts_project` is.

### `ng_package`

* Each label of the `deps` of an `ng_package` names a Bazel package (`//pkg`, `//pkg:target`, `@//pkg`; `:target`
  and `target` the `BUILD` file's own). When that package lies in the npm package whose `BUILD` file holds the call
  and holds `index.ts`, that file is a production entry, whose exports are public. A label of another repository or
  of a package outside the npm package names none.

### Relative imports into unbuilt build output

* A relative import that resolves to nothing and names a path inside a package's build output (an `outDir` of its
  build directories, or a first directory named like build output: `dist`, `lib`, ...) resolves to the source that
  path maps to, as a published path maps ([ADR 0035](0035-plugin-entries-in-build-output.md)). A file inside that
  output directory names no build output, so its own relative imports are never mapped.
* An import that maps to no source is judged as before (generated when ignored, else `unresolved-imports`).

### Consequences

* Good, because the spec files and helpers a Bazel test target builds are used, and the members only they read are
  read.
* Good, because the secondary entry points of an Angular package are its public API, so their exports are not
  reported.
* Good, because a source a package loads through its own unbuilt output is reached, with what it imports.
* Bad, because a file a test target's `exclude` leaves out, or that lies in a Bazel subpackage the `glob` does not
  cross, is still a used build source.
* Bad, because a production target's `glob` still names nothing, so its files need an import or an entry.
* Bad, because only `index.ts` is taken for an `ng_package` entry point; a package built from another file needs an
  `entry`.

### Confirmation

* `crates/skif-js-plugins/src/bazel.rs`: `calls_keep_the_include_patterns_of_their_globs_and_their_true_arguments`.
* `crates/skif-js-plugins/tests/plugins.rs`:
  `bazel_ng_packages_name_their_entry_points_and_test_only_globs_are_build_sources`, and
  `bazel_build_files_credit_linked_dependencies_and_add_entries`, whose production `glob` still names nothing.
* `crates/skif-js/tests/fp_resolve.rs`: `ng_package_entry_points_and_globbed_test_only_specs_are_used`.
* `crates/skif-js/src/resolve.rs`: `a_relative_import_into_unbuilt_output_of_its_package_maps_to_source`;
  `crates/skif-js/tests/build_output_entries.rs`: `a_relative_require_into_unbuilt_output_reaches_its_source`.

## Pros and Cons of the Options

### Read the include patterns of the `glob` of test targets only

* Good, because test files are found as every test runner plugin finds them.
* Good, because the dead files a production `glob` matches are still reported.
* Bad, because `exclude` is not applied, so an excluded test file counts as used.

### Read every `glob` of every rule, with its `exclude` patterns

* Good, because a `glob` means what Bazel means by it.
* Bad, because a `ts_project` that builds `glob(["src/**/*.ts"])` makes every file of the package used, and
  `unused-files` reports nothing in it.
* Bad, because an `exclude` applies to one target only, and the build sources of a package are one pattern list.

### Keep ADR 0031 and leave these files to an `entry` setting

* Good, because nothing changes.
* Bad, because every Bazel repository with specs and every Angular library needs the same configuration, and the
  member findings on them are wrong until it has it.
