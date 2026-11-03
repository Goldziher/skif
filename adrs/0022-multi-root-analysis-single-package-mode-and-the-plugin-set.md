---
status: accepted
date: 2026-11-03
decision-makers: ["Na'aman Hirschfeld"]
---

# Multi-Root Analysis, Single-Package Mode and the Plugin Set

## Context and Problem Statement

The accepted records describe one run as one workspace: [ADR 0001](0001-mission-and-scope.md) and [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) assume that `skif [PATH]` finds a single workspace root, and [ADR 0012](0012-reporting.md) defines a JSON document with one summary. The implementation does more. It lints a directory of independent projects in one run, it lints a plain `package.json` that declares no workspace, and its JS plugin table holds 25 plugins where [ADR 0011](0011-plugin-model.md) lists 21. None of this is recorded, so the behaviour is defined only by the code. This record states it.

## Decision Drivers

* A repository of independent projects must be lintable with one command and one exit code
* An ancestor workspace must not claim projects it does not list as members
* A project that is not a monorepo must be linted without inventing a workspace
* The plugin list in the records must equal the plugin table in the code

## Considered Options

* Record the multi-root and single-package behaviour and the 25-plugin table as the contract
* Remove multi-root runs and require one invocation per project
* Leave the behaviour undocumented

## Decision Outcome

Chosen option: "Record the multi-root and single-package behaviour and the 25-plugin table as the contract", because the behaviour exists, is tested, and removing it would make `skif .` fail on repositories that hold several independent projects.

**Root of a single-workspace run.** `find_workspace_root` takes the start path, or its directory when it is a file, and requires it to be absolute. The project is the nearest ancestor, the start directory included, that holds a `package.json` or declares a workspace. A directory declares a workspace when it holds a `pnpm-workspace.yaml` or its `package.json` has a `workspaces` key. If the project declares a workspace it is the root. Otherwise the root is the nearest ancestor that declares a workspace and lists the project among its members after negations; when no ancestor does, the project is its own root. An ancestor workspace therefore claims only the members it declares. When neither the start directory nor any ancestor holds a `package.json` or a workspace declaration, the lookup fails with `NoWorkspace`.

**Root discovery.** `discover_roots` in `skif-workspace` walks the start directory and returns the roots below it sorted by path. The walk does not enter `node_modules`, hidden directories, symbolic links or directories excluded by a `.gitignore` or `.ignore` found at or below the start directory, and it stops at a depth of 32 by default (`DiscoverOptions::max_depth`). Ancestors of the start directory are not consulted. A directory is a `RootKind::Workspace` when it declares a workspace and is not a member of an outer workspace found by the walk. A directory is a `RootKind::Package` when it holds a `package.json`, declares no workspace, is not a member of a found workspace and does not lie below a member. A workspace nested in an outer one that does not list it as a member is a root of its own, and so is a project below a workspace whose patterns do not select it. A workspace whose manifest cannot be parsed is still a root; linting it reports the problem. `discover_all` in the `skif` crate loads each root as a `Target`, in parallel, and returns them in path order.

**Per-root configuration.** `load_target` searches for `skif.toml` or `.skif.toml` from the root upwards and stops at a ceiling. In `discover_all` the ceiling is the start directory, so a root takes its nearest configuration file at or below the start directory and a root with none gets the default configuration, never one from above the start directory. A single-workspace run uses the root itself as the ceiling, so only the root's own file counts. Roots with equal configuration share one `Linter`. Diagnostic locations of a configuration file inherited from a parent directory are relative to the root and use `..` segments.

**Choosing a multi-root run.** `skif -r` and `--recursive` select a multi-root run, and so does a start path with no `package.json` and no workspace declaration at or above it, which makes `skif .` at the top of a repository of independent projects lint each of them. The `recursive` option of the napi addon selects the same mode. A run that finds no root fails with the workspace error kind and the message `no workspace or package found at or below <path>`. A relative start path to `discover_all` or `find_workspace_root` fails with `RelativePath`.

**Reporting.** Diagnostics of a multi-root run are merged into one report in the usual deterministic order, the package counts are summed, and every path is relative to the invocation directory. Only a multi-root run adds a `roots` array to the JSON document. Each entry has `path` (the root relative to the invocation directory, `.` for the invocation directory itself), `packages` (the package directories of the root, relative to the invocation directory, the root package first) and `summary` (the summary of that root's report). The existing fields keep their meaning, and a single-workspace document has no `roots` field. Human output is grouped by root. `--fix` plans, writes and reports per root.

**Exit code.** The exit code of a multi-root run is the worst code over the roots. Without `--max-warnings`, each root applies its own `fail-on-warnings`. With `--max-warnings`, the bound applies to the warnings of the whole run. Roots are linted in parallel, results keep path order, and the first failure in path order is the one reported, so the outcome does not depend on scheduling.

**Single-package mode.** `WorkspaceSource::SinglePackage` means that nothing declares a workspace and the root package is the only package. Workspace discovery yields it in two cases: no workspace declaration exists, and the declaration is the implicit one of a `pnpm-workspace.yaml` that holds only settings (no `packages:` key) when pnpm's default patterns, `.` and `**`, find no package besides the root. A declared `workspaces` or `packages:` list that matches no member keeps its declared source. In single-package mode `root-package-private-field` and `root-package-dependencies` do not report, because a standalone package is published and keeps its dependencies in its own `package.json`. `root-package-manager-field` is not affected. The other rules run unchanged.

**Plugin set.** The JS plugin table in `skif-js-plugins` holds 25 plugins. The first 21 are those listed in [ADR 0011](0011-plugin-model.md). The other four are `rspack`, `mastra`, `aws-cdk` and `browser-extension`. In table order the 25 are `typescript`, `eslint`, `prettier`, `vite`, `vitest`, `jest`, `next`, `storybook`, `playwright`, `tailwind`, `postcss`, `babel`, `webpack`, `rspack`, `rollup`, `tsup`, `turbo`, `husky`, `lint-staged`, `commitlint`, `changesets`, `github-actions`, `mastra`, `aws-cdk` and `browser-extension`. The list in [ADR 0011](0011-plugin-model.md) is the original set, not the current one; the table is the current one. A plugin beyond the list is added the way [ADR 0011](0011-plugin-model.md) describes: a source file under `crates/skif-js-plugins/src/plugins/`, an entry in the static table in `registry.rs`, and a fixture that fails without the plugin and passes with it. The table's length is a fixed-size array, so adding a plugin changes the length in the same change.

### Consequences

* Good, because one command and one exit code cover a repository of independent projects.
* Good, because an ancestor workspace cannot absorb a project it does not list.
* Good, because a plain package is linted without root-workspace rules that do not apply to it.
* Good, because the records and the plugin table agree.
* Bad, because the JSON document has two shapes, with and without `roots`, and consumers must branch on its presence.
* Bad, because the start directory bounds configuration lookup in a multi-root run, so a `skif.toml` above it is ignored there but honoured in a single-workspace run.
* Bad, because the walk skips symbolic links and hidden directories, so a project reachable only through either is not found.

### Confirmation

* The `skif-workspace` tests for `discover_roots` cover sibling roots in path order, members that are never roots, a nested workspace outside the member set, a project below a workspace that does not list it, skipped directories, the start directory as a root and the depth bound.
* `skif --format json -r` on a directory with two projects emits a `roots` array of two entries, and the same command on one workspace emits none.
* A run in which one root has errors and another is clean exits with the problems code.
* A `package.json` without `workspaces` and a `pnpm-workspace.yaml` without `packages:` over a single package both discover as `SinglePackage`, and the two `root-package-*` rules named above report nothing for them.
* `skif_js_plugins::plugins().len()` is 25 and the ids equal the list above.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Record the multi-root and single-package behaviour and the 25-plugin table as the contract

* Good, because the records describe what the tool does.
* Bad, because the contract now includes the second JSON shape.

### Remove multi-root runs and require one invocation per project

* Good, because the JSON document keeps one shape.
* Bad, because a repository of independent projects needs a wrapper script and the caller must combine the exit codes.

### Leave the behaviour undocumented

* Good, because it needs no work.
* Bad, because the behaviour is defined only by the code and can change without a record.

## More Information

* Amends: [ADR 0011](0011-plugin-model.md), [ADR 0012](0012-reporting.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0004](0004-configuration-model.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0017](0017-release-policy-a-single-complete-0-1-0.md)
* See the [ADR index](index.md).
