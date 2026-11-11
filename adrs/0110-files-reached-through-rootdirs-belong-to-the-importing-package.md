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
* The common layouts keep their owners: a file of a package's own directory stays that package's
* The graph stays deterministic, built in the same order whatever the scheduling

## Considered Options

* The package of the importing file adopts the file
* The package of the equivalent path below the importer's own root directory
* Map every build output tree to its package by configuration

## Decision Outcome

Chosen option: "The package of the importing file adopts the file", because `rootDirs` merges the directories into
one virtual tree whose files are the importer's own, and the importer's owner is already known when the edge is
lowered, in either direction between the root directories.

* The resolver marks an outcome found through `rootDirs` (`Outcome::root_dirs`). When the graph reaches such a file
  for the first time, the importing module's package owns it instead of the package of its location.
* A module so adopted passes its owner on: a file it reaches first by a relative import whose location falls in the
  same package as the adopted module's own location (generated code beside generated code) is adopted too. A bare or
  aliased import, or a relative one into another package's directory, keeps the owner of the target's location.
* A module keeps the owner it got when first reached; the graph is built in rounds merged in a fixed order, so the
  first importer is the same in every run.
* An adopted file outside its owner's directory matches none of the owner's `ignore` or project globs, and is not a
  file of another project.

### Consequences

* Good, because the imports of generated code reached through `rootDirs` use the dependencies of the package that
  imports it: no `unlisted-dependencies` finding at the root and no unused dependency in the package.
* Good, because `--trace-dependency` names the owning package for such a file.
* Neutral, because a `rootDirs` pair within one package directory changes nothing: the importer's owner is the
  owner by location.
* Bad, because a generated file imported first from one package and later from another stays with the first; the
  layouts that motivate this give each package its own output tree.

### Confirmation

* `crates/skif-js/tests/fp_resolve.rs` (`files_reached_through_root_dirs_belong_to_the_importing_package`): a
  gitignored `bazel-bin` file reached through the `rootDirs` of `packages/app` and the file it imports beside it use
  app's `zod` and `uuid`, with no unlisted or unused dependency.
* `crates/skif-js/src/resolve.rs` (`relative_imports_cross_root_dirs`): an outcome found through `rootDirs` is marked,
  a direct relative one is not.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### The package of the importing file adopts the file

* Good, because it needs no path arithmetic and holds in both directions between root directories.
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
