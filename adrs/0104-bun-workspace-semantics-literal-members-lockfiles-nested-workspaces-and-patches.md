---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0005", "0089", "0091"]
---

# Bun Workspace Semantics: Literal Members, Its Two Lockfiles, Nested Workspaces and Patches

## Context and Problem Statement

[ADR 0005](0005-workspace-discovery.md) expands the patterns of every package manager with pnpm's semantics, because
the managers "differ only at edges skif does not need to reproduce". It detects Bun from `packageManager`,
`devEngines` or a `bun.lock` / `bun.lockb`. [ADR 0089](0089-stray-lockfiles-and-nested-workspace-roots.md) accepts both
of Bun's lockfiles at the root, and reports only a nested `pnpm-workspace.yaml` as a nested workspace root.
[ADR 0091](0091-stale-patched-dependencies.md) reads the patches of pnpm only.

Real Bun installs (Bun 1.4.4) differ from these assumptions at four points:

* A directory listed by its literal path (`"packages/literal"`) stays a member even when a negation
  (`"!packages/literal"`) matches it. A negation only removes what a glob selected. npm removes the directory.
* With both `bun.lock` and `bun.lockb` at the root, Bun installs from `bun.lock` and ignores the binary file. The
  binary file then only goes stale.
* Bun ignores the `workspaces` of a member's `package.json`. The sub-members it names are never installed or linked.
* Bun applies the top-level `patchedDependencies` of the root `package.json` and records each applied patch in the
  `patchedDependencies` of `bun.lock`.

A Bun repository that commits no lockfile can also hold nothing but a `bunfig.toml`, which no step of detection
reads. Repositories installed by npm or Yarn commit a `bunfig.toml` too, only to configure `bun test` or the runtime.

Which of these does skif follow, and where?

## Decision Drivers

* The members skif lints are the members the detected manager installs
* A finding names something the manager does: a file it ignores, a member it never links, a patch it does not apply
* No change for a workspace whose manager is not Bun
* Lockfiles are read on demand, by the rules that need them ([ADR 0090](0090-lockfile-scopes-for-banned-groups-and-dependency-families.md))

## Considered Options

* Follow Bun where its behaviour differs, keyed on the detected package manager
* Keep pnpm's semantics for every manager

## Decision Outcome

Chosen option: "Follow Bun where its behaviour differs, keyed on the detected package manager", because a Bun
workspace is otherwise linted with members Bun does not install, and its ignored lockfile, never-installed
sub-workspaces and unapplied patches go unreported.

**Literal members.** When the detected manager is Bun, a positive pattern without a glob character names a directory
that no negation removes. The negation still removes every other directory it matches, and its match count leaves the
literal directory out. Any other manager keeps the expansion of ADR 0005.

**Detection.** A root `bunfig.toml` with an `[install]` table, which configures Bun's installer, detects Bun as the
last step, after the lockfiles, `pnpm-workspace.yaml` and the Yarn Berry files. A repository with only such a
`bunfig.toml` is a Bun repository; one with another manager's lockfile or files keeps that manager. A `bunfig.toml`
without an `[install]` table (only `[test]`, `preload` or other runtime keys), or one that does not parse, does not
decide.

**`stray-lockfile`.** Under Bun, a root `bun.lockb` next to a `bun.lock` is reported, because Bun ignores it. A
`bun.lockb` alone is still the lockfile Bun reads.

**`nested-workspace-root`.** Under Bun, a selected member whose `package.json` declares `workspaces` is reported at
that key, because Bun installs none of the directories it lists. A member `[workspace] ignore-packages` excludes is not
reported, as for a `pnpm-workspace.yaml`. Yarn and npm members with `workspaces` are not reported.

**`stale-patched-dependencies`.** Under Bun, the entries are the top-level `patchedDependencies` of the root
`package.json`, and the lockfile is `bun.lock` (`skif_workspace::BunLockfile`, read only by this rule). A key's
release must be one `bun.lock` resolves, and `bun.lock` must record the key in its `patchedDependencies`; otherwise
the entry is reported as not applied. The binary `bun.lockb` is not read: with no `bun.lock`, with only a `bun.lockb`
or with a `bun.lock` that does not parse, the rule reports that once and still checks the files, as ADR 0091 has it.

### Consequences

* Good, because the members, the stray lockfile, the nested workspaces and the patches of a Bun workspace match what
  `bun install` does.
* Good, because every other manager keeps its behaviour, so no existing report changes. An npm workspace that
  commits no lockfile and keeps a `bunfig.toml` only for `bun test` is not taken for Bun, so it gets no `workspace:`
  fix that npm cannot install.
* Bad, because the expansion now depends on the detected manager. A Bun repository that skif detects as another
  manager, or does not detect, keeps the negation of ADR 0005. That includes a Bun repository whose only sign is a
  `bunfig.toml` without an `[install]` table.
* Neutral, because a `bun.lockb`-only workspace gets no lockfile check of its patches until it moves to `bun.lock`,
  Bun's default since 1.2.

### Confirmation

* `crates/skif-workspace/fixtures/bun` holds the `bun.lock` that `bun install` (1.4.4) wrote. Its `workspaces` list
  the members discovery returns (`tests/discovery.rs`).
* `crates/skif-workspace/tests/memory.rs`: a literal member kept under Bun and removed under npm, with the negation
  counts of each, and the `bunfig.toml` cases of the detection precedence, with and without an `[install]` table.
* `crates/skif-rules/src/manifest/stray_lockfile.rs`, `nested_workspace_root.rs` and
  `stale_patched_dependencies.rs` tests: the Bun cases, and the Yarn and npm cases that stay unreported.

## Pros and Cons of the Options

### Follow Bun where its behaviour differs, keyed on the detected package manager

* Good, because skif's members and findings match the install.
* Bad, because discovery and three rules carry a Bun branch.

### Keep pnpm's semantics for every manager

* Good, because one expansion serves every manager.
* Bad, because a member that Bun links is not linted, and a stale `bun.lockb`, a never-installed sub-workspace and a
  patch Bun does not apply are not reported.

## More Information

* Bun workspaces: <https://bun.com/docs/install/workspaces>
* Bun lockfile: <https://bun.com/docs/install/lockfile>
* Bun patches: <https://bun.com/docs/install/patch>
* `bunfig.toml`: <https://bun.com/docs/runtime/bunfig>
