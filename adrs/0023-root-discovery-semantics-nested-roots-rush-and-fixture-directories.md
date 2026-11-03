---
status: accepted
date: 2026-11-03
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0022"]
---

# Root Discovery Semantics: Nested Roots, Workspaces Without a Root Manifest, Rush and Fixture Directories

## Context and Problem Statement

[ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) records how `skif -r` finds roots and which configuration each root gets. Running it on public monorepos showed behaviour it does not cover or states differently from the code. `root-package-manager-field` fired on hundreds of example and fixture packages below one repository root. A directory with a `pnpm-workspace.yaml` and no `package.json` failed with exit code 2. A Rush repository, whose members are listed in `rush.json`, was not a workspace at all. Test fixture trees produced roots that nobody wanted to lint. This record states the rules that now apply and amends the points of ADR 0022 that changed.

## Decision Drivers

* A recursive run over a large repository must not drown real findings in findings about its examples and fixtures
* A directory that declares a workspace must be linted as one, whether or not it holds a root `package.json`
* One bad root must never hide the others
* Discovery must not depend on files outside the repository being linted
* Every default that hides a directory must be visible in the configuration and replaceable

## Considered Options

* Record the semantics below as the contract
* Report `root-package-manager-field` for every root and let users silence it with `[[ignore]]`
* Treat every `package.json` below the start path as a root, nested or not, with no fixture skipping

## Decision Outcome

Chosen option: "Record the semantics below as the contract", because the alternatives make a recursive run unusable on the repositories it exists for: the first buries the findings that matter, the second removes the guarantee that a workspace owns its members.

**Configuration ceiling.** The search for `skif.toml` or `.skif.toml` from a root upwards stops at the root of the git repository that holds the root, the nearest directory with a `.git` entry (a directory, or the file of a linked worktree). Outside a repository it stops at the root itself in a single-workspace run and at the start directory in a multi-root run. A project therefore gets the same configuration whichever way it is reached inside a repository. This replaces the statement in ADR 0022 that the start directory is the ceiling of every multi-root run: a configuration file between the repository root and the start directory now applies, and a configuration file above the repository root never does. Workspace lookup from a start path likewise never climbs above the repository root.

**A nested workspace is always its own root.** A directory that declares a workspace is a root wherever it lies: when an outer workspace lists it as a member, when it lies below a member, and when no workspace lists it. The outer workspace leaves such a directory to its own root (`Workspace::ignore_dirs`), so each package is linted by exactly one root. This replaces the member-of-an-outer-workspace case of ADR 0022, where the outer workspace linted the nested workspace's root package as a member.

**Per-root failures are data.** A root that cannot be loaded (an invalid manifest, a configuration error, a broken `pnpm-workspace.yaml`) does not abort a multi-root run. The other roots are linted, the failure is reported to the user and, in the JSON document, as an entry of the `errors` array with `path` (relative to the invocation directory), `kind` and `message`, and the exit code is 2. A workspace that cannot be loaded still claims the directories its patterns declare, so their packages are not linted a second time as roots. A single-workspace run has one root, so its failure is the run's error as before.

**`root-package-manager-field` in a recursive run.** The rule asks about the root manifest of the project whose package manager is being decided. Below another root that decision belongs to the outer one. In a multi-root run a standalone package (a root that is not a workspace) with another root among its ancestor directories is marked nested (`Workspace::is_nested`), and the rule does not report for it. The rule still reports for every workspace, nested or not, and for every standalone package that has no root above it, which includes the outermost root, each of several sibling projects and a package linted on its own. A single-root run never marks anything nested. Every other rule is unaffected.

**Workspaces without a root manifest.** A directory is a workspace when it holds a `pnpm-workspace.yaml` or a `rush.json` or its `package.json` has a `workspaces` key. The `package.json` is no longer required. Without one the workspace is discovered with an empty stand-in for the root package, `Workspace::has_root_manifest` is false, and the stand-in is excluded from analysis exactly as an `ignore-packages` match would exclude it: no rule reports on it, it is not listed among the packages of the root and it is not counted in the summary. Members, catalogs and `pnpm-workspace.yaml` rules work as before. Rules that judge the root manifest therefore skip such a root. A directory with none of the three declarations and no `package.json` still fails with `MissingRootManifest`. The facade resolves a start path, a recursive run and a nested lookup through the same `declares_workspace`, so all three agree.

**Rush.** `rush.json` is a workspace declaration, read as JSONC. Each `projects[].projectFolder` is a member directory, relative to the file, and is matched as a literal pattern; the package manager is named by `pnpmVersion`, `npmVersion` or `yarnVersion`. Precedence for patterns is `workspaces` in `package.json`, then `pnpm-workspace.yaml`, then `rush.json`; for the package manager it is the `[workspace] package-manager` setting, then `packageManager`/`devEngines.packageManager`, then `rush.json`, then lockfiles. `WorkspaceSource::RushJson` names the source. A `projectFolder` that matches no directory is reported by `unmatched-workspace-patterns`, which offers no fix for it because removing a project from `rush.json` is not an edit to a pattern list. An invalid `rush.json` fails its root like an invalid `package.json`. Nothing else of Rush is read: its own configuration directory, shrinkwrap files and `common/` scripts are out of scope.

**Fixture directories.** Root discovery does not enter directories that match `[workspace] discover-ignore`, so no root is found at or below them. The default is `__fixtures__`, `fixtures`, `__mocks__` and `__snapshots__`. The patterns use gitignore syntax relative to the start directory: a bare name matches at any depth, a pattern with a slash is anchored at the start directory, and a leading `!` re-includes. Setting the key replaces the default list and `discover-ignore = []` skips nothing; there is no `extend-` form because a user who wants the defaults plus one name writes them out. The start directory is never skipped and the setting is read from the configuration file that governs the start directory, because discovery runs before any root has a configuration. The setting concerns root locations only. It does not stop a workspace from listing a member inside such a directory, and it does not affect which files the JS analysis reads. Names such as `test/e2e` are deliberately not in the default: they hold real projects in some repositories, and the setting is the place to add them.

### Consequences

* Good, because a recursive run reports a package manager finding once per repository and not once per example.
* Good, because pnpm and Rush repositories without a root `package.json` lint instead of failing.
* Good, because the default skips the directories that hold fixtures in nearly every JavaScript repository, and the user can see and replace the list.
* Good, because a configuration file outside the repository never changes a result.
* Bad, because `discover-ignore` is read from the file at the start directory, so a `skif.toml` in a subdirectory cannot change what the walk skips.
* Bad, because a real project that lives in a directory named `fixtures` is not found by default and must be enabled with `discover-ignore`.
* Bad, because a package that is nested under a root by accident, for example a published example, loses the package manager check even though it is linted.
* Bad, because the workspace without a root manifest carries an empty stand-in package that every consumer of `Workspace` must know is excluded.

### Confirmation

* The `roots` tests in `skif-workspace` cover a `rush.json` workspace, a broken one, the directories skipped by default, a custom list, `!` re-inclusion and the start directory named like a fixture directory.
* The `skif-workspace` tests for `Workspace` cover a `pnpm-workspace.yaml` and a `rush.json` without a root manifest, the Rush members, spans and package manager, and the configuration override of the Rush package manager.
* The `targets` tests in `skif` cover which roots are nested, a single root that is never nested, the workspace file without a root manifest in a recursive run and through `find_target`, and the fixture list from the configuration.
* `skif -r --format json` on a tree with a workspace, a standalone example below it and a nested workspace reports `root-package-manager-field` for the workspaces and not for the example; a `pnpm-workspace.yaml` alone lints its members with an empty `errors`.
* `skif schema` equals `schema/skif.schema.json` and lists `discover-ignore`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Record the semantics below as the contract

* Good, because the behaviour of a recursive run is stated in one place and checked by tests.
* Bad, because the contract includes a default that hides directories.

### Report `root-package-manager-field` for every root and let users silence it with `[[ignore]]`

* Good, because no rule needs to know about nesting.
* Bad, because every user of a large repository must write the same suppression, and the report is unusable until they do.

### Treat every `package.json` below the start path as a root, nested or not, with no fixture skipping

* Good, because discovery is trivial to explain.
* Bad, because members would be linted twice and fixture trees would produce roots with findings nobody can act on.

## More Information

* Amends: [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md)
* Related decisions: [ADR 0004](0004-configuration-model.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* See the [ADR index](index.md).
