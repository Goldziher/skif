---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0087", "0089", "0107"]
---

# Bazel Convenience Links, Module Pins and More Lockfile Consumers

## Context and Problem Statement

Three Bazel cases are still handled outside what Bazel itself does:

* A `*` or glob segment of a workspace pattern follows links to directories, as pnpm's does. After a build, the
  `bazel-<name>` link at the root points to the execroot, which mirrors the root, so `packages: ['*']` makes it a
  member. It repeats every root finding under `bazel-<name>/` and adds a `nested-workspace-root` finding for the
  mirrored `pnpm-workspace.yaml` (GitHub issue #174). pnpm lists the same member, so dropping it would depart from pnpm.
* [ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md) reads Node and
  package-manager pins from version files, Dockerfiles and workflows. rules_js installs pnpm under Bazel from
  `pnpm.pnpm(pnpm_version = ...)` or `npm_translate_lock(pnpm_version = ...)`, and rules_nodejs installs Node from
  `node.toolchain(node_version = ...)`. A `MODULE.bazel` that pins pnpm 8 while `packageManager` says pnpm 9 gives no
  finding (GitHub issue #175).
* [ADR 0107](0107-stray-lockfiles-a-build-installs-from-and-a-dependency-in-none-of-several-catalogs.md) has
  `stray-lockfile` read only the files Bazel reads for a lockfile and list no directory. It names its own gaps: a
  lockfile only another package's `BUILD` file names, a label inside a `genrule` command, a `glob()`, a Dockerfile in
  another directory whose context is the member, and a docstring that is exactly a label (GitHub issues #198, #199).

How does skif report a member inside a convenience link, read Bazel's toolchain pins, and find those consumers,
without running Bazel?

## Decision Drivers

* Discovery keeps pnpm's member semantics
* A finding states the cause once instead of repeating findings in a copy of the tree
* skif reads files and evaluates nothing: no Bazel query, no Starlark interpreter
* The extra cost is paid only where the old answer was a finding, never by a workspace outside a Bazel repository
* Library crates read through `FileSystem` only

## Considered Options

* Keep link members, exclude them from analysis and report them once; read Bazel pins as a new toolchain kind; list
  the Bazel repository's `BUILD` files only for a lockfile the files Bazel reads leave unnamed
* Skip `bazel-*` links in pattern expansion; leave Bazel pins and the other consumers to `allow` entries
* Honour `.bazelignore` and Bazel's package boundaries everywhere by walking the whole Bazel repository up front

## Decision Outcome

Chosen option: "Keep link members, exclude them from analysis and report them once; read Bazel pins as a new
toolchain kind; list the Bazel repository's `BUILD` files only for a lockfile the files Bazel reads leave unnamed",
because it keeps pnpm's semantics and the walk-free reading of ADR 0107 for every lockfile it already resolves, and
spends a listing only where the rule would otherwise report a lockfile a build installs from.

**Convenience links.** Discovery records as `DiscoveryFacts::bazel_links` each member whose first path segment is a
symbolic link to a directory of the root named `bazel-*`; the root is listed for this only when such a member exists.
Those members stay in the workspace and are excluded from analysis as `[workspace] ignore-packages` would exclude
them, so no rule repeats the root's findings under them. `nested-workspace-root` reports each once, at its
`package.json`, with the detail `BazelConvenienceLink` and `!bazel-*` as the pattern to add, and no longer reports the
`pnpm-workspace.yaml` or Bun `workspaces` inside it. A real directory named `bazel-*` is an ordinary member.

**Bazel pins.** The toolchain rules get a kind `bazel`: `MODULE.bazel`, `WORKSPACE.bazel` and `WORKSPACE` at the
workspace root, and through `files` also `.bzl` and `*.MODULE.bazel` files. They are lexed as Starlark (strings,
names and brackets; comments skipped), and a string literal passed as `pnpm_version` directly to a call whose name
ends in `pnpm` or `npm_translate_lock` pins pnpm, and one passed as `node_version` to a call whose name ends in
`toolchain` or `nodejs_register_toolchains` pins Node. A computed value pins nothing. `toolchain-pin-drift` reads the
kind by default and checks those pins against the root as any other pin; the label in a message is the call and the
argument (`pnpm.pnpm pnpm_version`). `foreign-package-manager-command` does not read it by default.

**More lockfile consumers.** In a Bazel repository, besides the consumers of ADR 0107:

* a label inside a longer string literal counts where `$(location <label>)`, `$(locations ...)`, `$(execpath ...)`,
  `$(execpaths ...)`, `$(rootpath ...)`, `$(rootpaths ...)`, `$(rlocationpath ...)` or `$(rlocationpaths ...)`
  expands it, as in a `genrule`'s `cmd`;
* a `glob(...)` (also `native.glob(...)`) in the `BUILD` file of the lockfile's package names it when one of its
  include patterns (the first list, or `include`) matches the lockfile's path relative to the package and none of its
  exclude patterns (the second list, or `exclude`) does, with Bazel's `*` and `**`. The package is the nearest
  directory at or above the lockfile's that holds a `BUILD` file read, so a glob does not reach into a subpackage;
* a string literal that is the first statement of a file or of a `def` is a docstring and names nothing;
* when the files Bazel reads for the lockfiles leave one unnamed, the Bazel repository is listed for its `BUILD` and
  `BUILD.bazel` files, without entering hidden directories, `node_modules`, symbolic links (the convenience links) or
  the directories its `.bazelignore` names, and those files are read with the Starlark files they load, transitively,
  each once. A file that names no lockfile, calls no `glob(` and names no `.bzl` or `*.MODULE.bazel` file is not
  lexed.

A Dockerfile in any directory also names a lockfile when a build gives it the lockfile's directory as its context and
it copies the lockfile from there or copies the whole context and then installs with the lockfile's manager: a
`docker build`, `docker buildx build`, `docker image build` or `podman build` in a shell script or workflow the
toolchain scan reads, with a static context and optional `-f`/`--file`, both taken from the workspace root; or a
service's `build` in a Compose file at the root (`compose.yaml`, `compose.yml`, `docker-compose.yaml`,
`docker-compose.yml`), whose `dockerfile` is relative to its `context`. Dockerfiles are judged before Bazel files, and
the Bazel files are searched only for the lockfiles no Dockerfile copies.

### Consequences

* Good, because a build no longer fills the report with copies of root findings under `bazel-<name>/`, and the one
  finding left says how to remove the link from the patterns.
* Good, because a pnpm or Node version Bazel installs that differs from the one outside Bazel is reported.
* Good, because the unsafe fix of `stray-lockfile` no longer deletes a lockfile that another package's `BUILD` file, a
  macro, a `genrule` command, a `glob()` or a Dockerfile built from the member uses, and none of those needs an `allow`
  entry.
* Bad, because a Bazel repository with a member lockfile nothing names pays a listing of its directories on every run
  until the lockfile is removed or allowed.
* Bad, because a link member is excluded from analysis even when the user meant to lint it; `!bazel-*` is the
  supported layout.
* Neutral, because a script that changes directory before `docker build` is read as if it ran from the root.
* Neutral, because only the root's Bazel files are read by default for pins; a Bazel repository above the workspace,
  or a pin in a loaded `.bzl` file, needs a `files` glob.

### Confirmation

* `crates/skif-workspace/src/bazel.rs`: `members_inside_a_bazel_convenience_link_are_facts_and_ignored` and
  `a_real_directory_named_like_a_link_is_an_ordinary_member`.
* `crates/skif-rules/src/manifest/nested_workspace_root.rs`: `a_member_inside_a_bazel_convenience_link_is_reported_once`
  and `a_real_directory_named_like_a_bazel_link_is_judged_as_any_member`.
* `crates/skif-rules/src/starlark.rs` tests; `crates/skif-rules/src/toolchain/parse.rs`:
  `bazel_files_pin_through_rules_js_and_rules_nodejs_calls`; `crates/skif-rules/src/toolchain/pin_drift.rs`:
  `bazel_module_pins_are_checked_against_the_root`.
* `crates/skif-rules/src/manifest/lockfile_consumers.rs`: docstrings, location functions, globs and their matching,
  `when_the_files_bazel_reads_name_every_lockfile_no_directory_is_listed`,
  `a_lockfile_only_another_package_names_is_found_by_listing_the_build_files`,
  `a_glob_reaches_only_into_its_own_package`, `the_listing_skips_hidden_and_ignored_directories_and_node_modules` and
  the Dockerfile cases with build contexts.
* `crates/skif-rules/src/toolchain/docker_builds.rs` tests: build commands, Compose services and path normalization.
* `crates/skif-rules/src/manifest/stray_lockfile.rs`: `a_lockfile_only_another_packages_build_file_names_is_not_reported`,
  `a_glob_of_the_lockfiles_package_names_it` and
  `a_dockerfile_elsewhere_built_from_the_members_directory_names_its_lockfile`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Keep link members, exclude them from analysis and report them once; read Bazel pins as a new toolchain kind; list the Bazel repository's `BUILD` files only for a lockfile the files Bazel reads leave unnamed

* Good, because members match pnpm and the findings match what a build reads.
* Bad, because the reading of Starlark is lexical: a label or version built from variables is not seen.

### Skip `bazel-*` links in pattern expansion; leave Bazel pins and the other consumers to `allow` entries

* Good, because it changes the least code.
* Bad, because skif's members would differ from pnpm's, and every rules_js monorepo would restate its build in
  `skif.toml`.

### Honour `.bazelignore` and Bazel's package boundaries everywhere by walking the whole Bazel repository up front

* Good, because one listing answers every question.
* Bad, because every run in a Bazel repository pays a full listing, also when the files Bazel reads already name every
  lockfile.

## More Information

* Amends: [ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md) (the `bazel` kind),
  [ADR 0089](0089-stray-lockfiles-and-nested-workspace-roots.md) (`nested-workspace-root` reports a member inside a
  convenience link) and
  [ADR 0107](0107-stray-lockfiles-a-build-installs-from-and-a-dependency-in-none-of-several-catalogs.md) (the
  consumers above, and a listing of `BUILD` files when a lockfile is left unnamed)
* Issues: [#174](https://github.com/Goldziher/skif/issues/174), [#175](https://github.com/Goldziher/skif/issues/175),
  [#198](https://github.com/Goldziher/skif/issues/198), [#199](https://github.com/Goldziher/skif/issues/199)
* Bazel location expansion: <https://bazel.build/reference/be/make-variables>
* Bazel `glob`: <https://bazel.build/reference/be/functions#glob>
* rules_js `npm_translate_lock`: <https://github.com/aspect-build/rules_js/blob/main/docs/npm_translate_lock.md>
* See the [ADR index](index.md).
