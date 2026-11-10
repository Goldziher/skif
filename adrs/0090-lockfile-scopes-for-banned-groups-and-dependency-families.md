---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0001"]
---

# Lockfile Scopes for Banned Groups and Dependency Families, Read from pnpm-lock.yaml on Demand

## Context and Problem Statement

[ADR 0009](0009-version-and-catalog-policies.md) judges the dependency instances the manifests declare, and
[ADR 0001](0001-mission-and-scope.md) put lockfile analysis out of scope: "skif reasons about declared specifiers, not
resolved trees". Two common policies are about the resolved tree.

* **Banned packages anywhere in the graph.** A copyleft-licensed or known-bad package arrives transitively and is kept
  out with an `overrides` entry. `banned-dependency` sees only declarations, so removing the override brings the
  package back and the lint still passes.
* **Families that must resolve to one version.** `playwright`, `playwright-core` and `@playwright/test` resolved to
  different releases install mismatched browser builds; `@types/react` should resolve once per React major.
  `unsynced-dependency-family` compares what one package declares, not what the lockfile resolves.

Can skif judge what the lockfile resolves without making every run read it, and without passing silently where it
cannot read it?

## Decision Drivers

* Rule IDs are the public contract ([ADR 0007](0007-rule-and-diagnostic-model.md)): a ban stays a `banned-dependency`
  and a family stays an `unsynced-dependency-family`
* The configuration keeps syncpack's shape ([ADR 0009](0009-version-and-catalog-policies.md),
  [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md)), and first match wins
  ([ADR 0040](0040-version-groups-own-their-instances-the-catalog-rules-leave-claimed-instances.md))
* A run whose configuration asks for no lockfile check reads no lockfile
* Library crates read files only through the `FileSystem` trait ([ADR 0003](0003-library-first-embedding.md))
* A check that cannot run says so; a lockfile that does not parse is an error, not a pass

## Considered Options

* A `scope` on version groups and dependency families, judged on a pnpm lockfile model the rules read on demand
* New rule IDs (`banned-resolved-dependency`, `lockfile-version-groups`)
* Lockfile resolutions as dependency instances, under every version policy

## Decision Outcome

Chosen option: "A `scope` on version groups and dependency families, judged on a pnpm lockfile model the rules read on
demand", because it keeps the rule IDs and the group shape, reads the lockfile only when a group asks for it, and
leaves the instance model of ADR 0009 alone.

**Lockfile model.** `skif-workspace` models `pnpm-lock.yaml` in lockfile format 9 (pnpm 9 and 10): the importers with
their direct dependencies (section, specifier, resolved version reference), the keys of `packages:`, the snapshots
with name, version, the `patch_hash` of the key's suffix and the keys of their `dependencies` and
`optionalDependencies`, `patchedDependencies` (pnpm 9's `hash` and `path`, pnpm 10's bare hash) and `overrides`.
Fields it does not model are ignored. A version reference that names another package (`string-width@4.2.3`) is an
alias and resolves to that snapshot; a `link:` is a workspace package, an importer of its own, and is not followed.
`PnpmLockfile::resolutions(importer)` walks the snapshots an importer reaches breadth-first, so each has a shortest
chain, ties going to the dependency listed first. Spans of snapshot, importer and patch entries come from the same
line scanner as the `overrides` entries of `pnpm-workspace.yaml` (`YamlSource::entry_span`). A file that is not YAML,
has an unexpected shape (a section that is not a mapping, a snapshot key without `@version`) or is not format 9 is
`WorkspaceError::InvalidLockfile`. The YAML budget grows with the text, so a lockfile of millions of nodes parses and a
document of aliases expanding far beyond its size is still refused. `yarn.lock`, `package-lock.json`, `bun.lock` and
older pnpm formats are not modelled.

**Read on demand.** Discovery does not read the lockfile. The rules read it through `LintContext::fs`, the file system
a host passes with `RunOptions::with_fs` (the real one by default; `Linter` passes the one it was built with), and share
one parse per run through `LintContext::shared`. A configuration without a lockfile-scoped group or family reads
nothing.

**Banned groups.** A `[[version-groups]]` entry takes `scope = "manifests"` (default) or `scope = "lockfile"`. A
lockfile-scoped group is valid only with `policy = "banned"`; `dependency-types` and `specifier-types` select manifest
entries and are rejected with it. It claims no manifest instance, so those fall through to the groups after it and the
catalog rules treat it as absent. `banned-dependency` reports each resolved `name@version` its `dependencies` match once
(the peer variants of a release are one finding), at its `snapshots:` entry, with the importer and the shortest chain
that pulls it in (`importer packages/app pulls it in through mid@2.0.0 > bad@1.0.0`), the importer's package, the
dependency and version, and the detail `IsBanned`. `packages` chooses the importers by the workspace package they
belong to. A direct dependency is reported too: in the lockfile scope a declaration is a resolution like any other.

**Families.** A `[[dependency-families]]` entry takes `scope = "manifests"` (default), `"importer"` or `"lockfile"`,
and with a lockfile scope an optional `anchor`. In `importer` scope the members each importer reaches, in `lockfile`
scope every snapshot of the lockfile, must be on one version: each lower release is reported at its snapshot, naming
the first of the highest releases. With an `anchor`, each member must be on one version per major, and a member on a
major the anchor does not resolve to is reported. The exemptions of the `manifests` scope (0.x members of a released
family, floating specifiers) do not apply: a resolved version is never floating. A family in a lockfile scope is not
compared on the manifests. Moving a built-in family to a lockfile scope replaces it as any entry of its name does,
with its members listed. The summaries of both rules now name the lockfile.

**No silent pass.** When a lockfile check cannot run, its rule reports why at its usual severity: there is no
`pnpm-lock.yaml` (at the configuration file, or for patches at the first entry), the workspace has another manager's
lockfile instead (at that file), or the lockfile does not parse (at the lockfile, with the parser's message).

### Consequences

* Good, because a transitive ban and a family split fail the lint where the lockfile shows them, with the path that
  leads there.
* Good, because the rule IDs, the group shape and the instance model stay as they were, and a run that asks for no
  lockfile check costs nothing.
* Bad, because only pnpm is covered; a yarn, npm or bun workspace gets a diagnostic that nothing was checked.
* Bad, because a ban matches by name, not by version range, and a group does not follow `link:` to judge what another
  workspace package pulls in for its dependents (that package is an importer of its own and is judged there).

### Confirmation

* `skif-workspace` unit tests in `lockfile.rs`: importers, aliases and links, snapshots, both `patchedDependencies`
  shapes, shortest chains, spans, and the errors for another format, a section that is not a mapping and broken YAML.
* `skif-lint` test `rules_read_through_the_file_system_the_host_passes`.
* `skif-config` test `lockfile_scopes_are_read_and_checked`.
* `skif-rules` unit tests in `policy/lockfile_bans.rs` (a transitive ban with its chain, removing the override that
  kept a package out, a direct dependency, the `packages` selector, a missing or foreign lockfile, a lockfile that
  does not parse, no read without a lockfile group) and `policy/lockfile_families.rs` (a member split in one importer
  while the manifests scope reports the manifest, the lockfile scope, anchor majors, a missing or broken lockfile).
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A `scope` on version groups and dependency families, judged on a pnpm lockfile model the rules read on demand

* Good, because the configuration that bans or groups a package is where its lockfile check is switched on.
* Bad, because a lockfile-scoped group behaves differently from its neighbours: it claims no instance.

### New rule IDs (`banned-resolved-dependency`, `lockfile-version-groups`)

* Good, because the lockfile checks could be turned on and off on their own.
* Bad, because a ban would be configured in a group and reported under a second ID, and `[[ignore]]` entries and
  baselines naming `banned-dependency` would not cover it.

### Lockfile resolutions as dependency instances, under every version policy

* Good, because every policy (pinned, same-range, snapped-to) would apply to resolved versions.
* Bad, because the instance model, the fixes, `skif list` and the syncpack comparison all assume an instance is a
  declaration that can be edited; a resolution cannot.

## More Information

* Amends: [ADR 0001](0001-mission-and-scope.md) (lockfile analysis is in scope for these checks, through
  `pnpm-lock.yaml` format 9)
* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md),
  [ADR 0009](0009-version-and-catalog-policies.md),
  [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md),
  [ADR 0040](0040-version-groups-own-their-instances-the-catalog-rules-leave-claimed-instances.md),
  [ADR 0091](0091-stale-patched-dependencies.md)
* Issue: [#141](https://github.com/Goldziher/skif/issues/141)
* See the [ADR index](index.md).
