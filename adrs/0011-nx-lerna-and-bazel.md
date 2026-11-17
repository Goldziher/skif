---
status: accepted
date: 2026-11-17
decision-makers: ["Na'aman Hirschfeld"]
---

# Nx, Lerna and Bazel

## Context and Problem Statement

Repositories built with Nx, Lerna or Bazel keep part of what skif looks for outside `package.json` and tool configs.
An integrated Nx project is a directory with a `project.json` and no manifest: the root lists every dependency while the
project keeps its own tool configs, and target options name entries relative to the workspace root. Lerna reads its
package list from `lerna.json` and fails on configuration its installed release rejects. These repositories also hold
layouts that are there by design: task-ordering dependencies, grouping directories and `turbo prune` output. In a Bazel
repository with rules_js, dependencies are `node_modules` labels, entries and build sources are named by rules in
Starlark, generated code exists only in output trees, and toolchain pins and lockfile consumers live in `BUILD` and
module files. skif itself also runs under `bazel run` and `bazel test`.

How do the plugins see an Nx project that is not a package, what does skif read of Lerna, which layouts does it
recognise without configuration, and how does it read a Bazel repository and run inside one?

## Decision Drivers

* Projects and build facts come from what Nx, Lerna and Bazel themselves read; no new configuration
* The package manager's declaration stays the one source of members
  ([ADR 0004](0004-workspaces-roots-and-package-managers.md))
* Starlark is read statically and never evaluated, as tool configs are
  ([ADR 0008](0008-plugins-config-evaluation-and-commands.md))
* Findings do not depend on what was built last, and a missing import nothing generates stays a finding
* Outside these tools nothing changes, and the extra cost is paid only where it changes an answer

## Considered Options

* Nx projects as scopes of their package, Lerna read as facts, layouts recognised by shape, and Starlark scanned as text
  with output trees never read
* Treat each Nx project as a workspace package of its own, because a project lists no dependencies, so every package
  its configs name would be unlisted there and unused at the root
* Let `lerna.json` declare the members whenever it exists, because the package manager installs its own members
* Run `bazel query` or evaluate Starlark, because it needs Bazel or an interpreter and a fetched repository, takes
  seconds to minutes and cannot run in the library

## Decision Outcome

Chosen option: "Nx projects as scopes of their package, Lerna read as facts, layouts recognised by shape, and Starlark
scanned as text with output trees never read", because the plugins then read a project exactly as they read a package
while findings stay with the manifest that lists the dependencies, every layout is decided by files skif already reads,
and a text scan finds what rules_js repositories write without running Bazel.

### Nx projects and scopes

* Projects are found only when the `nx` plugin is enabled for the package. Every `project.json` is listed once per run,
  with the source walk's rules ([ADR 0007](0007-js-analysis-core-on-oxc.md)), and owned by the closest package above it.
* A project directory with no `package.json` in it or between it and its package is a scope. Every other enabled plugin
  also runs there, with the scope as its directory and the package's dependencies, and its output is rebased onto the
  package. An entry expression is matched only below its scope, and a config of a scope that hides entries hides them
  only in `<scope>/**`.
* A plugin without default globs that finds no config in a scope applies its config-less conventions there only when a
  target of the scope's `project.json` runs its Nx executor.
* A project with its own `package.json` belongs to that package: a member runs its plugins itself, and a non-member
  stays outside the analysis.

### Nx target options and local plugins

* Target option paths are read relative to the workspace root and rebased onto the package. `{workspaceRoot}` and
  `{projectRoot}` are substituted as Nx substitutes them; another token or an absolute path is not read.
* `main` and `additionalEntryPoints` are production entries, and the `*Config` options are configs of their plugins.
  `run-commands` commands are read as commands run in their `cwd`
  ([ADR 0008](0008-plugins-config-evaluation-and-commands.md)).
* Nx's `getJestProjects()` and `nxE2EPreset()` are recognised in the source and resolve to what Nx computes, instead of
  making the package incomplete.
* A plugin, executor, `targetDefaults` key or generator names a local module when its specifier is a workspace-relative
  path or matches a `paths` pattern of the root TypeScript config. The module is an entry, and so is every
  implementation and factory its executor, builder, generator and schematic collections name, because a collection is
  the plugin's public interface.

### Lerna

* Discovery reads a valid `lerna.json` beside the root manifest and records it as a fact: its `packages`,
  `useWorkspaces`, its `version`, and the directories only one of its list and the declared members selects. Its
  `packages` and `npmClient` decide the members and the manager only when nothing else declares them.
* `stale-lerna-config` reports configuration and commands the installed Lerna release rejects or has removed; the
  release comes from the installed package, else the lowest version the root range admits, and without one nothing is
  reported.
* `lerna-packages-mismatch` reports when Lerna's list and the declared members select different packages, unless
  `useWorkspaces` is set.

### Layouts recognised by shape

* A dependency on a member with no module entry field and no code file is not reported unused: nothing can import or
  run it, so it orders tasks.
* `missing-package-manifest` skips a selected directory that holds a `project.json`, or holds only directories the
  patterns also select.
* The root search of a recursive run does not enter the `json` and `full` trees that `turbo prune` writes beside a
  lockfile in a directory without a `package.json`; the start directory itself is always searched.

### Bazel repositories

* The Bazel root is the nearest directory with a module or workspace file, searched no higher than the git repository
  root. The `bazel` plugin is enabled by a `BUILD` file at the top of a package, and reads the `BUILD` files below it
  only when a root file names `aspect_rules_js`.
* Starlark is scanned as text and never evaluated. A string literal that is a `node_modules/<name>` label or another
  repository's `<name>/package_json.bzl` names a dependency of the label's Bazel package, and a dependency a package's
  own `BUILD` file names that its manifest does not list is unlisted there, at the literal's line. One fixed list
  comprehension shape (one `%s`, no `if`) expands to its labels. Templates, `select()`, variables and macros name
  nothing.
* The literal `entry_point` of the binary, devserver and test rules is an entry. The literal sources and data of
  `js_library`, `ts_project` and their kin are build sources: the module graph starts from them, but their exports are
  judged as any module's. Build sources reach nested members and nested roots as plugin entries do, and
  `skif explain-usage` names them `build-source`.
* A target with `testonly = True` is a test target, as `js_test` is: its literal files count only outside
  `--production`, and so do the include patterns of a `glob` in its sources, without its `exclude` patterns. The
  `glob` of a production target names nothing, so a dead production file stays `unused-files`.
* Each Bazel package an `ng_package` names in its `deps` within the same npm package publishes its `index.ts` as a
  production entry with public exports.

### Bazel output trees and generated code

* `bazel-bin`, `bazel-out`, `bazel-testlogs` and `bazel-<workspace>` at the Bazel root are never read. An import that
  resolves there is generated code, and so is a missing relative import that the nearest `BUILD` file declares among a
  call's outputs or output directories. Any other missing import stays `unresolved-imports`.
* A workspace member whose first path segment is a `bazel-*` symbolic link stays in the workspace but is excluded from
  analysis, and `nested-workspace-root` reports it once with the pattern to add. A real `bazel-*` directory is an
  ordinary member.

### Bazel toolchains, lockfiles and hoisting

* The `bazel` toolchain kind reads literal pnpm and Node versions from the rules_js and rules_nodejs calls in the root
  module files, and `toolchain-pin-drift` checks them
  ([ADR 0006](0006-version-policies-lockfiles-toolchains-and-updates.md)).
* A member lockfile a build installs from is not stray. Its consumers are labels in the files Bazel reads for it,
  `$(location ...)` expansions and a `glob` of its own package. Only when those leave it unnamed are the repository's
  `BUILD` files listed, from the git index under git discovery.
* rules_js `public_hoist_packages` make their packages available where they are hoisted
  ([ADR 0004](0004-workspaces-roots-and-package-managers.md)).

### Running under `bazel run` and `bazel test`

* When `BUILD_WORKSPACE_DIRECTORY` is set and not empty, the CLI changes to `BUILD_WORKING_DIRECTORY` (else the
  workspace directory) before parsing arguments, so relative paths mean what they mean in the user's shell, and
  `bazel run //:skif -- --fix` fixes the source tree. A directory it cannot enter exits `2`. The npm package's `skif`
  bin does the same; the addon's programmatic API never changes directory. The library reads no environment.
* Under `bazel test` (`TEST_TMPDIR` set and not empty), a cache without an explicit location keeps its store below
  `$TEST_TMPDIR` ([ADR 0007](0007-js-analysis-core-on-oxc.md)), so nothing is written into the output tree.

### Consequences

* Good, because an integrated workspace's tool configs and the packages they name are read, and a computed config in
  one project leaves only that project unjudged.
* Good, because a `lerna.json` the installed Lerna rejects is reported, while skif's members stay the ones the package
  manager installs.
* Good, because task-ordering dependencies, grouping directories, Nx projects and prune output produce no findings.
* Good, because Bazel findings are the same before and after any build, and an output the `BUILD` files declare is
  never unresolved.
* Bad, because each scope runs the plugins its files call for once more, and the scope machinery is a second way
  plugins see a directory.
* Bad, because labels, entries and outputs that a macro, a variable or a filtered comprehension declares are not found,
  and a test target's `exclude` is not applied.
* Bad, because a stale dependency on a member without a module entry and without code is not reported.
* Bad, because the `bazel run` directory rule is written twice, in Rust and in JavaScript.

### Confirmation

* `crates/skif-js-plugins/tests/nx_projects.rs`, `crates/skif-js-plugins/tests/projects.rs` and
  `crates/skif-js/tests/nx_integrated.rs` cover scopes, target options and local plugins.
* `crates/skif-workspace/src/lerna.rs`, `crates/skif-rules/src/manifest/stale_lerna_config.rs` and
  `crates/skif-rules/src/manifest/lerna_packages_mismatch.rs` cover Lerna.
* `crates/skif-js/tests/task_ordering_dependencies.rs`, `crates/skif-rules/src/manifest/missing_package_manifest.rs`
  and `crates/skif-workspace/src/roots.rs` cover the layouts recognised by shape.
* `crates/skif-js-plugins/src/bazel.rs`, `crates/skif-js-plugins/tests/plugins.rs` and
  `crates/skif-js/tests/fp_resolve.rs` cover the Starlark scan, build sources and generated outputs.
* `crates/skif-workspace/src/bazel.rs`, `crates/skif-rules/src/toolchain/parse.rs`,
  `crates/skif-rules/src/manifest/lockfile_consumers.rs` and `crates/skif-js/tests/public_hoisting.rs` cover
  convenience links, pins, lockfile consumers and hoisting.
* `crates/skif-cli/src/env.rs`, `crates/skif-cli/tests/cli.rs` and `crates/skif-cli/tests/cache.rs` cover `bazel run`
  and `bazel test`, and `scripts/npm-smoke.sh` runs the npm bin under `bazel run`.
