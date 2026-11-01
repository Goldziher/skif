---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Workspace Discovery: Facts From Four Package Managers, Decisions in Rules

## Context and Problem Statement

Every check skif runs starts from the same question: which packages make up this workspace, and
what does each one declare? The answer depends on the package manager:

* **npm, yarn and bun** read a `workspaces` field in the root `package.json` — an array of globs,
  or (yarn classic, bun) an object with a `packages` array.
* **pnpm** ignores that field and reads `packages:` from `pnpm-workspace.yaml`, which also holds
  the default `catalog:` and named `catalogs:`.
* **Catalogs** exist in pnpm (`pnpm-workspace.yaml`) and in bun and yarn (`catalog` / `catalogs`
  in the root `package.json`, or under `workspaces`).

The patterns are not plain globs. pnpm treats `*` as direct children only and `**` as "any depth";
`node_modules` and hidden directories are never packages; `!pattern` excludes. The inherited `collect.rs` already learned the `**`
behaviour the hard way (`find_nested_packages`, `expand_nested_star`) and carries fixtures for it
(`fixtures/pnpm-glob`).

The inherited code also mixed discovery with judgement: collection decides that a pattern matching
nothing is an error and exits the process (`std::process::exit(1)` in `collect.rs`), so the rule
that reports it (`non-existant-packages`) and the discovery that observes it are the same code.

## Decision Drivers

* Every check starts from which packages make up the workspace and what each declares
* npm, yarn, bun and pnpm differ in where patterns and catalogs live, and the patterns are not plain globs
* The inherited code mixed discovery with judgement and exited the process
* Embedded hosts supply their own buffers, and output must be deterministic

## Considered Options

* Fact-only discovery in skif-workspace with pnpm pattern semantics for every package manager
* A general glob library for workspace patterns
* Shelling out to `pnpm ls -r --json` / `npm query`
* Per-manager pattern semantics

## Decision Outcome

Chosen option: "Fact-only discovery in skif-workspace with pnpm pattern semantics for every package manager", because discovery that only reports facts keeps judgement in ordinary, configurable rules, tolerates a broken member manifest, and gives every rule and fix spans captured once.

`skif-workspace` discovers the workspace and returns **facts**; it never decides that a fact is a
problem. Turning a fact into a diagnostic is a rule's job ([ADR 0007](0007-rule-and-diagnostic-model.md)).

**Package-manager detection** produces an `Option<PackageManager>`, first match wins:

1. `[workspace] package-manager` in `skif.toml` ([ADR 0004](0004-configuration-model.md)).
2. The root `package.json` `packageManager` field (`pnpm@9.12.0`), then `devEngines.packageManager`
   (an object or a list; the first known entry wins).
3. Lockfile presence, in this order: `pnpm-lock.yaml`, `bun.lock` / `bun.lockb`, `yarn.lock`,
   `package-lock.json`.
4. `pnpm-workspace.yaml` presence implies pnpm.
5. Otherwise **`None`**: nothing indicates a manager, and discovery does not guess npm.

`PackageManager` is the one enum defined in `skif-config` and used by discovery, rules and fixes;
`skif-workspace` holds the single detection function. A rule or fix that depends on the manager
(`workspace:` versus npm's plain ranges, where a catalog lives, which install command to run)
treats `None` as "unknown" and does not emit a fix that would be wrong for some manager; in
particular an npm workspace never receives a `workspace:*` specifier. The field is preferred over
lockfiles because it is the project's declared intent and carries a version; a stray lockfile from
another manager is common in migrating repositories. Discovery returns the winning manager; a rule
may compare it with the lockfiles on disk.

**Pattern expansion** follows pnpm semantics for every manager, because npm, yarn and bun accept the
same pattern shapes and differ only at edges skif does not need to reproduce:

* `dir/*` — direct children of `dir` that contain a `package.json`; a segment may mix literal text
  and wildcards (`apps/web-*`), and a pattern such as `apps/*/web` matches only `web` directories
  that contain a `package.json`.
* `dir/**` — `dir` itself if it has a `package.json`, and every directory below it that has one,
  **descending through packages** (a package nested inside another package is still a member),
  as pnpm's `**/package.json` glob does. `dir/**/*` excludes `dir` itself. Traversal stops at a
  fixed depth limit so a symlink cycle cannot hang discovery.
* `!pattern` — removes matches; negations apply after all positive patterns, regardless of order.
* `node_modules` and dot-directories are never entered.
* Paths are handled as root-relative strings with `/` separators, so results do not depend on how
  the root was spelled or on the platform.

Discovery returns a `Workspace` value holding, among others:

* the root package and each member `Package`, addressed by a `PackageId` (its index in the sorted
  package list) rather than by display name, so two packages with the same or no `name` stay
  distinct; each holds the manifest path, name, version, `private`, `packageManager`, `engines`,
  and **every dependency entry with its byte span** in the manifest ([ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md)), so diagnostics
  point at the exact entry and fixes can edit it. The spans come from a **span table recorded during
  the single parse of the manifest**, not from re-parsing per lookup;
* `DependencyInstance` values — one per (package, dependency kind, name, specifier) — each holding an
  `Arc<Specifier>` **parsed during discovery** with `skif-specifier` ([ADR 0006](0006-specifier-and-semver-model.md)), which is what policies
  iterate over ([ADR 0009](0009-version-and-catalog-policies.md)). `DependencyKind` is defined once and shared with configuration;
* catalogs from all sources (pnpm, the root `package.json`, bun's `workspaces` object), with their
  origin file and spans;
* per-pattern match results, including patterns that matched nothing and directories matched
  without a `package.json`;
* **`manifest_errors`**: members whose manifest could not be read or parsed, as typed data
  (`WorkspaceError` with path and 1-based line and column; a missing manifest is distinct from an
  invalid one), not as an early exit. The rest of the workspace is still discovered, and the
  `package_manager` is `None` rather than a guessed value when nothing indicates one.

All reads go through the `FileSystem` trait defined in `skif-core` (`read_to_string`, `read_dir`,
`is_file`, `is_dir`, with the file type taken from the directory entry rather than a second stat),
so discovery runs against an in-memory tree in tests and against a host's buffers when embedded
([ADR 0003](0003-library-first-embedding.md)). `skif-workspace` also provides the workspace-root search the CLI and hosts use to
start from any directory ([ADR 0004](0004-configuration-model.md)). Manifests are read and parsed in parallel, one per rayon
task, each task parsing its specifiers into a thread-local map that is merged once at the end;
the output is sorted by path, so results are deterministic regardless of thread scheduling.

### Consequences

* Good, because `unmatched-workspace-patterns` and `missing-package-manifest` ([ADR 0007](0007-rule-and-diagnostic-model.md)) are ordinary rules over
  facts; they can be configured, ignored and fixed like any other.
* Good, because A broken `package.json` in one member is a diagnostic for that member; the rest of the workspace
  is still linted.
* Good, because spans captured once during discovery serve every rule and every fix without re-parsing.
* Bad, because applying pnpm `**` semantics to npm and yarn workspaces can differ from what those managers
  install in unusual layouts (a package nested inside another package). This is accepted as the
  more predictable rule and is covered by fixtures.
* Bad, because detection order is a heuristic. A repository with no `packageManager` field and two lockfiles is
  resolved by lockfile order (pnpm, bun, yarn, npm); the rule that recommends a `packageManager` field exists to make that
  case rare. Returning `None` instead of guessing npm means some fixes are withheld in a workspace
  that gives no signal at all.
* Bad, because capturing spans for every dependency entry costs a CST-aware JSON parse instead of
  `serde_json::from_str`; the 50 ms budget on 500 packages ([ADR 0001](0001-mission-and-scope.md)) includes it.

### Confirmation

* `cargo test -p skif-workspace` runs the discovery tests against `MemoryFs` and against the fixtures (including `fixtures/pnpm-glob`): `**` descending below packages, negations in any order, partial-segment globs, `apps/*/web`, bun `workspaces.catalog`, and root-relative `/` paths.
* A detection test table in `skif-workspace` asserts the order config override, `packageManager`, `devEngines.packageManager`, lockfiles (pnpm, bun, yarn, npm), `pnpm-workspace.yaml`, and `None` when nothing is present.
* A discovery test with one broken member `package.json` asserts the error appears in `manifest_errors` with its line and column, that the other members are still discovered, and that a missing manifest is a different error from an invalid one.
* A determinism test runs discovery on a generated 500-package tree several times on rayon pools of different sizes and asserts identical `Workspace` values.
* `mise run bench` measures discovery plus manifest and policy rules on the generated 500-package monorepo against the 50 ms budget ([ADR 0001](0001-mission-and-scope.md)); span-table capture is inside the measured region.

## Pros and Cons of the Options

### Fact-only discovery in skif-workspace with pnpm pattern semantics for every package manager

* Good, because rules turn facts into diagnostics, so `unmatched-workspace-patterns` and `missing-package-manifest` are configurable like any other rule, and a broken member manifest does not stop the rest of the workspace from being linted.
* Good, because spans and specifiers are captured once and serve every rule and fix.
* Bad, because pnpm `**` semantics can differ from what npm or yarn install in unusual layouts, detection order is a heuristic, and span capture costs a CST-aware parse instead of `serde_json::from_str`.

### A general glob library for workspace patterns

* Good, because it is less code to maintain than a pattern walker.
* Bad, because standard globbing does not know about `node_modules`, hidden directories, the requirement of a `package.json` or negations applied after all positive patterns, and would enumerate every directory below a `**`.

### Shelling out to `pnpm ls -r --json` / `npm query`

* Good, because the package manager is the authority on its own workspace.
* Bad, because it needs the package manager installed and a populated `node_modules`, and is orders of magnitude slower.

### Per-manager pattern semantics

* Good, because each manager's edge cases would be exact.
* Bad, because it needs four implementations of nearly the same thing, for differences that appear only in layouts (packages nested in packages) that are themselves worth reporting.

## More Information

* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0009](0009-version-and-catalog-policies.md)
* See the [ADR index](index.md).
