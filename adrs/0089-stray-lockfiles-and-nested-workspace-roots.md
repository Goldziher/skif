---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0074"]
---

# Stray Lockfiles and Nested Workspace Roots, with Allow-Lists That Carry Reasons

## Context and Problem Statement

npm, Yarn, pnpm and Bun each install a workspace from one lockfile at its root. A `package-lock.json`, `yarn.lock`,
`npm-shrinkwrap.json`, `pnpm-lock.yaml` or `bun.lock` in a member, or another manager's lockfile at the root, is
almost always a leftover: nothing reads it except a tool started in that directory, which then installs from the wrong
source. A `pnpm-workspace.yaml` in a member is worse. pnpm takes the nearest one above the working directory, so an
install there silently leaves the root workspace (GitHub issue #140). skif reports neither.

Some projects keep such a file on purpose, for example a tool that a build system installs from its own
`package-lock.json`. Root discovery ([ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md),
[ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md)) makes any directory that
declares a workspace a root of its own, also when an outer workspace lists it, and a recursive run then excludes it
from the outer workspace's analysis.

Which files are reported, by which rules, how does a nested root that skif already lints on its own take part, and how
is a file kept on purpose?

## Decision Drivers

* A finding must name a file whose removal (or exclusion from the workspace) makes installs consistent
* The package manager the workspace uses decides which root lockfile is legitimate; skif does not guess one
* The multi-root model of ADR 0022 and ADR 0023 stays as it is
* Files git ignores are never reported, and library crates read only through `FileSystem`
* A file kept on purpose is recorded in the configuration with its reason, and an entry that no longer applies is
  reported, as `stale-js-config` reports `[js]` entries that match nothing

## Considered Options

* Two rules over the root and the selected directories, fed by discovery facts, with an `allow` option of entries
  with reasons
* One rule for lockfiles and nested `pnpm-workspace.yaml` files together
* Search the whole tracked tree through the git index of the JS analysis

## Decision Outcome

Chosen option: "Two rules over the root and the selected directories, fed by discovery facts, with an `allow` option
of entries with reasons", because the two findings have different causes, consequences and fixes, and the directories
workspace discovery already lists are the ones every package manager installs.

**The facts.** Workspace discovery lists the root and every directory the workspace patterns select (members, members
whose manifest is broken, and directories without a manifest), one directory listing each, and records the lockfiles
(`skif_workspace::LOCKFILES`: `package-lock.json`, `npm-shrinkwrap.json`, `yarn.lock`, `pnpm-lock.yaml`, `bun.lock`,
`bun.lockb`) and `pnpm-workspace.yaml` files found there in `DiscoveryFacts::package_manager_files`, relative to the
root and sorted. A file is left out when a `.gitignore` or `.ignore` between the root and its directory, one between
the repository root and the root, or `.git/info/exclude` excludes it or a directory above it; a negation re-includes
it, and the innermost file decides, as in git. Directories below a member that the patterns do not select are not
searched: a lockfile there belongs to a project the workspace does not install.

**`stray-lockfile`** (workspace, warning by default) reports every lockfile in a selected directory, and a lockfile at
the root that the detected package manager does not read: npm reads `package-lock.json` and `npm-shrinkwrap.json`,
pnpm `pnpm-lock.yaml`, Yarn `yarn.lock`, and Bun `bun.lock` and `bun.lockb`. Without a detected manager, root
lockfiles are not judged; detection already prefers the lockfile it finds first, so a root with two lockfiles and no
declaration reports the one detection did not pick. A member that holds its own `pnpm-workspace.yaml` is a root of its
own, so its lockfile is left to that root; so are members `[workspace] ignore-packages` excludes and members another
root of the run lints. The fix deletes the file and is unsafe ([ADR 0008](0008-fix-and-edit-model.md)): a lockfile
kept on purpose would be lost, and deleting a file also needs `--allow-remove-files`.

**`nested-workspace-root`** (workspace, warning by default) reports a `pnpm-workspace.yaml` in a selected directory,
under any package manager: the file makes pnpm, and skif's root discovery, treat the member as a workspace of its own.
The enclosing workspace reports it also when a recursive run lints the member as its own root, because that root
cannot see that it sits inside another workspace's patterns; a member `[workspace] ignore-packages` excludes is not
reported. A nested workspace outside the patterns is an independent project, as ADR 0023 has it, and is not reported.
There is no fix: the file may hold settings, catalogs or overrides that belong in the root file, which a deletion would
lose. A member `package.json` with `workspaces` is not reported: Yarn nests worktrees on purpose, and pnpm ignores the
field.

**Allow-lists.** Both rules take an `allow` option, a list of paths relative to the workspace root, each a string or a
`{ name, reason }` table: the entry type of the `[js]` ignore lists (`IgnoreItem`,
[ADR 0074](0074-reasons-on-js-ignore-entries.md)), with the path as `name`. A blank `reason` and an unknown key are
option errors. A leading `./` and backslashes are ignored when comparing. An entry that names none of the files the
rule would report without the list is reported by the same rule at the configuration file, with the message
``"`<name>` in `rules.<rule>.allow` names no stray lockfile"`` (or ``"names no nested `pnpm-workspace.yaml`"``). This is
skipped when the configuration file was inherited from a directory above the root (its relative path starts with
`..`): such a file configures every root below it, and an entry naming the file of another root is not stale here.
`--require-ignore-reasons` and `[lint] require-ignore-reasons` also fail on an entry of any rule's `allow` option
without a `reason`, named ``rules.stray-lockfile.allow[0] `tools/legacy/package-lock.json` ``. The issue's advice
stands in the help text: a project installed on its own is better excluded from the workspace patterns.

### Consequences

* Good, because a member's leftover lockfile and a member that silently forms its own pnpm workspace are reported,
  under every package manager skif supports.
* Good, because a file kept on purpose is recorded with its reason, the strict mode covers it, and an entry that no
  longer applies is reported.
* Good, because discovery lists each selected directory once and reads ignore files only for the files it found.
* Bad, because a lockfile that is untracked but not ignored is reported: discovery does not read the git index, which
  belongs to the JS analysis ([ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md)). Such a file
  is one `git add` from being committed and already misleads tools run in its directory.
* Bad, because a lockfile deeper inside a member, below a directory the patterns do not select, is not reported.
* Bad, because repositories with leftover lockfiles see new warnings; the fix or an `allow` entry clears them.
* Neutral, because an `allow` entry written in a configuration that nested roots inherit is judged only by the root
  that holds the file.

### Confirmation

* `crates/skif-workspace/tests/memory.rs`: `package_manager_files_of_the_root_and_the_selected_directories_are_listed`
  and `ignored_package_manager_files_are_not_listed`.
* `crates/skif-rules/src/manifest/stray_lockfile.rs` tests: the root lockfile of each manager, every lockfile in a
  member, another manager's lockfile at the root, no detected manager, a member with its own `pnpm-workspace.yaml`, an
  ignored member, a gitignored lockfile, allowed entries with and without a reason, stale entries, an inherited
  configuration, a blank reason and the fix.
* `crates/skif-rules/src/manifest/nested_workspace_root.rs` tests: a member's file reported and the root's not, any
  package manager, `ignore-packages`, a member another root lints, and allowed and stale entries.
* `the_first_suppression_without_a_reason_is_named_by_its_key` in `crates/skif-config/tests/config.rs` covers `allow`
  entries.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Two rules over the root and the selected directories, fed by discovery facts, with an `allow` option of entries with reasons

* Good, because each rule can be configured, ignored and fixed on its own, and a nested workspace root, which has no
  safe fix, does not hold back the lockfile fix.
* Good, because the manifest rules need no file system: the facts arrive with the workspace.
* Bad, because two rules carry the same option and the same staleness check.

### One rule for lockfiles and nested `pnpm-workspace.yaml` files together

* Good, because one switch and one allow-list cover both.
* Bad, because a nested workspace root is not a lockfile, its consequence is different, and it has no fix.

### Search the whole tracked tree through the git index of the JS analysis

* Good, because untracked files and files deep inside members would be judged exactly as git tracks them.
* Bad, because the manifest rules would depend on the JS analysis being enabled and built, and the index reader is
  private to `skif-js`.

## More Information

* Amends: [ADR 0074](0074-reasons-on-js-ignore-entries.md) (`--require-ignore-reasons` also covers the `allow`
  entries of a rule)
* Related decisions: [ADR 0008](0008-fix-and-edit-model.md),
  [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md),
  [ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md),
  [ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md)
* See the [ADR index](index.md).
