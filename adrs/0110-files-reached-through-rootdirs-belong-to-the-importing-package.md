---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
---

# Files Reached Through `rootDirs` Belong to the Importing Package

## Context and Problem Statement

The module graph gives every file the package whose directory is the closest above it. Build tools that write
generated code into a separate output tree break that rule. With rules_js, `packages/app/tsconfig.json` sets
`"rootDirs": [".", "../../bazel-bin/packages/app"]`, and `packages/app/src/index.ts` imports `./gen/schema.js`, a file
that exists only as `bazel-bin/packages/app/src/gen/schema.js`. The resolver finds it through `rootDirs`, but its
location falls in no package directory, so the graph gave it to the root. Its import of `zod` was then an
`unlisted-dependencies` finding at the generated file, and the `zod` that `@mono/app` lists was an
`unused-dependencies` finding, although Node.js resolves the generated file's imports from
`bazel-bin/packages/app/node_modules`, which holds app's dependencies.

Which package owns a file that the importer reaches through `rootDirs`, and the files that file imports in turn?

## Decision Drivers

* A file's imports are judged against the dependencies the runtime resolves them from
* The common layouts keep their owners: a file of a workspace package's directory stays that package's, whoever
  reaches it
* The graph stays deterministic, built in the same order whatever the scheduling

## Considered Options

* The package of the importing file adopts the file
* The package of the equivalent path below the importer's own root directory
* Map every build output tree to its package by configuration

## Decision Outcome

Chosen option: "The package of the importing file adopts the file", because `rootDirs` merges the directories into
one virtual tree whose files are the importer's own, and the importer's owner is already known when the edge is
lowered, whichever root directory the importer lies in.

* The resolver records the root directory below which it found an outcome through `rootDirs` (`Outcome::root_dirs`,
  such as `bazel-bin/packages/app`), whether it read `rootDirs` itself or the underlying resolver did. When the graph
  reaches such a file for the first time from a module of a workspace package, that package owns it instead of the
  root.
* Only a file of the root is adopted: one that lies in no workspace package's directory and is not a listed source
  file. A file of another workspace package, reached through `rootDirs` that point into its directory, stays that
  package's.
* A module so adopted passes its owner on: a file of the root it reaches first by a relative import below the same
  root directory (generated code beside generated code) is adopted too, and carries that root directory on. A bare or
  aliased import, or a relative one out of the root directory (into the output tree of another package, such as
  `bazel-bin/packages/lib`), keeps the owner of the target's location.
* A module keeps the owner it got when first reached; the graph is built in rounds from entries in path order, merged
  in a fixed order, so the first importer is the same in every run.
* An adopted file lies outside its owner's directory, so it matches none of the owner's `ignore` or project globs; the
  `ignore` globs of the root, where it lies, still apply. It is not a file of another project.

### Consequences

* Good, because the imports of generated code reached through `rootDirs` use the dependencies of the package that
  imports it: no `unlisted-dependencies` finding at the root and no unused dependency in the package.
* Good, because `--trace-dependency` names the owning package for such a file.
* Neutral, because a `rootDirs` pair within one package directory changes nothing: the importer's owner is the
  owner by location.
* Bad, because a generated file imported first from one package and later from another stays with the first; the
  layouts that motivate this give each package its own output tree.
* Bad, because a file of the root that is a listed source file is never adopted: a checked-in file that another
  package reaches through `rootDirs` is judged against the root's dependencies.

### Confirmation

* `crates/skif-js/tests/fp_resolve.rs` (`files_reached_through_root_dirs_belong_to_the_importing_package`): a
  gitignored `bazel-bin` file reached through the `rootDirs` of `packages/app` and the file it imports beside it use
  app's `zod` and `uuid`, with no unlisted or unused dependency.
* `crates/skif-js/tests/fp_resolve.rs`
  (`the_trace_of_a_dependency_names_the_package_that_adopted_the_importing_file`): `--trace-dependency` names
  `@mono/app` as the owner of the generated file and the package that uses `zod`.
* `crates/skif-js/tests/fp_resolve.rs` (`root_dirs_into_another_workspace_package_leave_its_files_with_it`): `rootDirs`
  into `packages/shared` leave its file with `@mono/shared`.
* `crates/skif-js/tests/fp_resolve.rs` (`relative_imports_out_of_the_root_dir_of_an_adopted_file_are_not_adopted`): a
  relative import from app's output tree into `bazel-bin/packages/lib` reaches a file of the root.
* `crates/skif-js/tests/fp_resolve.rs` (`the_first_package_to_reach_a_shared_generated_file_owns_it`): of two
  packages sharing one output tree, the first in path order owns the generated file.
* `crates/skif-js/tests/js_config.rs` (`files_adopted_through_root_dirs_keep_the_ignore_globs_of_the_root`): the
  root's `ignore` globs still apply to an adopted file.
* `crates/skif-js/src/resolve.rs` (`relative_imports_cross_root_dirs`,
  `another_root_dir_names_the_root_a_resolved_file_lies_below`): an outcome found through `rootDirs` names its root
  directory, a direct relative one names none.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### The package of the importing file adopts the file

* Good, because it needs little path arithmetic and holds whichever root directory the importer lies in.
* Good, because the files the generated code imports beside it can follow the same owner.
* Bad, because ownership now depends on how a file is first reached, not only on where it lies.

### The package of the equivalent path below the importer's own root directory

* Good, because it depends only on the importer's `tsconfig.json` and the specifier.
* Bad, because from a file of the output tree the equivalent path lies in the output tree again, so a source file
  reached back through `rootDirs` would be given to the root.
* Bad, because the files the generated code imports in turn are not reached through `rootDirs` and keep the root.

### Map every build output tree to its package by configuration

* Good, because ownership stays a property of the location.
* Bad, because every repository would have to spell out what its `tsconfig.json` files already say.

## More Information

* TypeScript `rootDirs`: <https://www.typescriptlang.org/tsconfig/#rootDirs>
* Issue: [#168](https://github.com/Goldziher/skif/issues/168)
