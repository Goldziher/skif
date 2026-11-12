---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0034"]
---

# Directories Matched by ignore-workspaces Leave the Packages Above Them

## Context and Problem Statement

[ADR 0034](0034-js-configuration-parity-with-knip.md) makes `[js] ignore-workspaces` the globs of workspace directories
the JavaScript analysis skips: the packages there contribute no files, entries or dependency findings. A glob such as
`playground/**` often covers more than its members: demos, scratch files and examples that belong to no package of their
own. Those files belong to the nearest package above them, usually the root, which still analyses them, so
`unused-files` reports them there.

knip's `ignoreWorkspaces` also takes the matched directories out of the workspaces above them: each ancestor workspace
adds the ignored patterns, ending in `/**`, as negated globs to its entry and project patterns. A knip configuration
with `ignoreWorkspaces: ["playground/**"]` reports nothing below `playground/`, and the same glob in `skif.toml`
reports the loose files there.

Should a directory that `ignore-workspaces` matches also leave the packages above it?

## Decision Drivers

* A migrated `ignoreWorkspaces` glob reports what knip reported with it
* A user who names a directory to skip expects nothing below it to be reported
* A workspace member below the directory that the globs do not match keeps its own analysis

## Considered Options

* A matched directory leaves every package above it
* Keep the narrower meaning, and have `skif migrate` add `ignore` globs for the root package

## Decision Outcome

Chosen option: "A matched directory leaves every package above it", because it is knip's meaning, so the key needs no
second translation, and it holds for configurations written for skif too.

A file is not analysed when a directory between its package's directory and the file, the package's own directory
excluded, matches an `ignore-workspaces` glob, matched as the members are: relative to the root (or to the start of
the run, [ADR 0098](0098-explicit-configuration-workspace-keys-and-shared-stale-settings.md)), with `*` stopping at `/`.
The file is no project file, no entry and no source of dependency findings in any package, as the files of a skipped
member are. A member below the directory that no glob matches is analysed as before, with its own files: knip applies
an ignored pattern only to the workspaces above it. The root directory is never matched. An entry that matches only
such a directory is no longer stale for `stale-js-config`.

### Consequences

* Good, because `ignore-workspaces = ["playground/**"]` leaves nothing below `playground/` reported, as in knip.
* Good, because `skif migrate` keeps translating `ignoreWorkspaces` key by key.
* Bad, because a glob that matched only members before now also hides the root's files in the directories it names;
  a project that wants them judged lists the members by their own directories.

### Confirmation

* `crates/skif-js/tests/js_config.rs`:
  `ignore_workspaces_takes_the_matched_directories_out_of_the_packages_above`, and
  `a_star_in_ignore_workspaces_does_not_cross_a_slash` for a nested member the glob does not match.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A matched directory leaves every package above it

* Good, because skif and knip agree on the key.
* Bad, because the key now hides files that are not in any skipped member.

### Keep the narrower meaning, and have `skif migrate` add `ignore` globs for the root package

* Good, because the key keeps the meaning ADR 0034 gave it.
* Bad, because the same glob means different things in the two tools, and a configuration written for skif needs a
  second key to skip a directory.
