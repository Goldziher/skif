---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Lerna Configuration, Task-Ordering Dependencies, Grouping Directories and Prune Output

## Context and Problem Statement

A review of skif on Turborepo, Nx and Lerna repositories found five places where skif either stays silent about
something the tool itself rejects or reports something that is there by design:

* Lerna 7 removed `useWorkspaces` from `lerna.json` and fails with `ECONFIGWORKSPACES` while it is set, Lerna 7 and
  later fail with `ENOVERSION` without a `version`, and Lerna 9 removed `lerna bootstrap`, `lerna add` and
  `lerna link`. skif reported none of it (GitHub issue #189).
* Lerna takes its package list from `packages` in `lerna.json` first, then from `pnpm-workspace.yaml` when `npmClient`
  is `pnpm`, then from the `workspaces` of `package.json`. skif reads `lerna.json` last, only when nothing else declares
  members. The two agree when one source exists and diverge silently when both do (#190).
* A workspace dependency on a member with no module entry and no code (a tooling, asset or style package) exists to
  order tasks, Turborepo's `^build` or Nx's task dependencies, and `unused-dependencies` reported it (#191).
* Globs such as `libs/*/*/*` in Nx repositories select grouping directories (`libs/cart`, whose content is the members
  `libs/cart/feature` and `libs/cart/ui`) and Nx projects with a `project.json` and no `package.json`, and
  `missing-package-manifest` reported each (#192).
* `turbo prune --docker` writes `out/json` (the manifests) and `out/full` (the pruned workspace) next to the pruned
  lockfile. When `out/` is not gitignored, a recursive run linted both trees as roots and reported the pruned packages
  again (#194).

What does skif follow, report and skip in each case?

## Decision Drivers

* The package manager installs and links the members, so its declaration stays the one skif lints
* A finding names what the tool in use rejects, at the line to change, and only for a release that rejects it
* No new configuration where the files already say enough; library crates read only through `FileSystem`
* Discovery records facts, rules judge them ([ADR 0005](0005-workspace-discovery.md))
* An exemption is narrow enough that a dependency that can be used is still judged

## Considered Options

* Keep the package manager's members and report what Lerna reads differently; exempt members nothing can import;
  recognise grouping directories, Nx projects and prune output by their layout
* Follow Lerna's `packages` whenever `lerna.json` declares them
* Make the task-ordering exemption an opt-in option
* Document the five cases and change nothing

## Decision Outcome

Chosen option: "Keep the package manager's members and report what Lerna reads differently; exempt members nothing
can import; recognise grouping directories, Nx projects and prune output by their layout", because each case is
decided by files skif already reads, and the package manager's declaration stays the one source of members.

**Lerna facts.** Discovery reads a valid `lerna.json` next to the root `package.json` also when another file declares
the members, and records it in `DiscoveryFacts::lerna`: its `packages` and where they are written, whether
`useWorkspaces` is set and where, and whether `version` is a string. When the members come from another file, it also
records the directories with a `package.json` that `packages` selects and the declaration does not, and the other way
round, with pnpm pattern semantics. `lerna.json` still declares the members only when nothing else does, and its
`npmClient` still names the package manager only then.

**`stale-lerna-config`** (workspace, warning, no fix) reports `useWorkspaces` (any value) and a missing `version` when
Lerna 7 or later is in use, and a root or member script that runs `lerna bootstrap`, `lerna add` or `lerna link` when
Lerna 9 or later is: directly, or through `npx`, `pnpx`, `bunx`, `npm`, `pnpm`, `yarn` or `bun` and their `exec`,
`dlx` and `x` subcommands. A pinned runner (`npx lerna@6 bootstrap`) runs that release and is not reported. The release
is the major version of `node_modules/lerna/package.json` at the root, else the lowest version the root `package.json`
range for `lerna` admits; a tag or `*` names none. Without a release nothing is reported: an older Lerna accepts all
of it.

**`lerna-packages-mismatch`** (workspace, warning, no fix) reports, at `packages` in `lerna.json`, when the directories
Lerna selects differ from the members skif lints, naming both lists and the directories only one of them selects.
Lists that are written differently but select the same packages are not reported. A `lerna.json` with
`useWorkspaces: true` is not reported: Lerna before 7 then follows the package manager, and `stale-lerna-config`
reports the key for later releases.

**Task-ordering dependencies.** `unused-dependencies` and `unused-dev-dependencies` do not report a dependency on a
workspace member whose `package.json` has none of `main`, `module`, `exports`, `bin`, `browser`, `types` and
`typings` and that owns no code file the analysis discovers. Nothing can import or run such a member, so the
dependency can only order tasks. A member with one of those fields or a code file is judged as before; when it is
listed only for task order, `ignore-dependencies` with a `reason` records that.

**Grouping directories and Nx projects.** A selected directory without a `package.json` is not in
`DiscoveryFacts::dirs_without_manifest`, and so not reported by `missing-package-manifest`, when it holds a
`project.json`, or when its content (hidden entries, `node_modules` and build output aside, as before) is only
directories the patterns select too. A directory that also holds a file, or a subdirectory the patterns do not select,
is still reported.

**Prune output.** The root walk of a recursive run does not enter the `json` and `full` directories of a directory
below the start path that has no `package.json` and holds a lockfile (`skif_workspace::LOCKFILES`) next to both. The
start directory itself is never treated so: `skif -r out` lints both trees.

### Consequences

* Good, because a `lerna.json` the installed Lerna rejects and scripts that call removed commands are reported before
  a release breaks on them.
* Good, because a repository whose Lerna list and package-manager workspaces disagree learns it, and skif's members
  stay the ones the package manager installs.
* Good, because task-ordering dependencies, Nx grouping directories, Nx projects and prune output no longer produce
  findings, without any configuration.
* Neutral, because `DiscoveryFacts` gains the `lerna` field and the reading of `lerna.json` on every discovery with a
  root manifest: one file read.
* Bad, because a stale dependency on a member without a module entry and without code is no longer reported; such a
  member can only be used through a path import, which the analysis credits when it sees one.
* Bad, because a Lerna installed outside the root `node_modules` (globally, or only in a member) and not declared at
  the root is not seen, so `stale-lerna-config` stays silent there.
* Bad, because a project's own `src` directory selected by a deep glob is still reported by
  `missing-package-manifest`: it holds files.

### Confirmation

* `crates/skif-workspace/src/roots.rs` tests: `the_trees_of_turbo_prune_output_are_not_searched_for_roots`,
  `json_and_full_directories_without_a_lockfile_beside_them_are_searched`,
  `a_package_with_a_lockfile_and_json_and_full_directories_is_no_prune_output` and
  `prune_output_given_as_the_start_directory_is_searched`.
* `crates/skif-rules/src/manifest/missing_package_manifest.rs`:
  `grouping_directories_and_nx_projects_need_no_manifest`.
* `crates/skif-js/tests/task_ordering_dependencies.rs`: members without a module entry or code, each module entry
  field, and a dependency that is no member.
* `crates/skif-workspace/src/lerna.rs` and `crates/skif-workspace/tests/memory.rs`: the Lerna facts, the directories
  only one list selects, lists that select the same packages, and `useWorkspaces`.
* `crates/skif-rules/src/manifest/stale_lerna_config.rs` and `lerna_packages_mismatch.rs` tests: each finding with its
  location, the release thresholds, the installed release over the declared range, runners and pinned runners, and
  the clean cases.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Keep the package manager's members and report what Lerna reads differently; exempt members nothing can import; recognise grouping directories, Nx projects and prune output by their layout

* Good, because every decision is read from files in the repository, with no option to learn.
* Good, because the member set does not depend on whether Lerna is used.
* Bad, because the layouts are heuristics: a directory that only groups members on purpose but also holds a README
  is still reported, and a directory named like prune output is skipped only with a lockfile next to it.

### Follow Lerna's `packages` whenever `lerna.json` declares them

* Good, because skif would lint what `lerna run` and `lerna publish` act on.
* Bad, because the package manager does not install the packages only Lerna lists, so their dependencies are not
  installed and imports of them do not resolve as skif would assume.
* Bad, because a leftover `lerna.json` would silently change the members of a repository that no longer uses Lerna.

### Make the task-ordering exemption an opt-in option

* Good, because no dependency stops being reported without the user asking.
* Bad, because the opt-in means configuration and a schema change for a case the manifest already decides, and
  `ignore-dependencies` already serves a member that has code.

### Document the five cases and change nothing

* Good, because no behaviour changes.
* Bad, because the false findings stay and the configurations Lerna rejects stay unreported.

## More Information

* Lerna configuration: <https://lerna.js.org/docs/api-reference/configuration>
* Turborepo `prune`: <https://turbo.build/repo/docs/reference/prune>
* Issues [#189](https://github.com/Goldziher/skif/issues/189), [#190](https://github.com/Goldziher/skif/issues/190),
  [#191](https://github.com/Goldziher/skif/issues/191), [#192](https://github.com/Goldziher/skif/issues/192) and
  [#194](https://github.com/Goldziher/skif/issues/194)
