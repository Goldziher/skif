# 0005 — Workspace Discovery: Facts From Four Package Managers, Decisions in Rules

* Status: Accepted
* Date: 2026-11-01

## Context

Every check skif runs starts from the same question: which packages make up this workspace, and
what does each one declare? The answer depends on the package manager:

* **npm, yarn and bun** read a `workspaces` field in the root `package.json` — an array of globs,
  or (yarn classic, bun) an object with a `packages` array.
* **pnpm** ignores that field and reads `packages:` from `pnpm-workspace.yaml`, which also holds
  the default `catalog:` and named `catalogs:`.
* **Catalogs** exist in pnpm (`pnpm-workspace.yaml`) and in bun and yarn (`catalog` / `catalogs`
  in the root `package.json`, or under `workspaces`).

The patterns are not plain globs. pnpm treats `*` as direct children only and `**` as "any depth,
stopping at the first directory that has a `package.json`"; `node_modules` and hidden directories
are never packages; `!pattern` excludes. The inherited `collect.rs` already learned the `**`
behaviour the hard way (`find_nested_packages`, `expand_nested_star`) and carries fixtures for it
(`fixtures/pnpm-glob`).

The inherited code also mixed discovery with judgement: collection decides that a pattern matching
nothing is an error and exits the process (`std::process::exit(1)` in `collect.rs`), so the rule
that reports it (`non-existant-packages`) and the discovery that observes it are the same code.

## Decision

`skif-workspace` discovers the workspace and returns **facts**; it never decides that a fact is a
problem. Turning a fact into a diagnostic is a rule's job (ADR 0007).

**Package-manager detection**, in order, first match wins:

1. `[workspace] package-manager` in `skif.toml` (ADR 0004).
2. The root `package.json` `packageManager` field (`pnpm@9.12.0`), name and version.
3. Lockfile presence: `pnpm-lock.yaml`, `bun.lock` / `bun.lockb`, `yarn.lock`,
   `package-lock.json`.
4. `pnpm-workspace.yaml` presence implies pnpm.
5. Otherwise npm.

The field is preferred over lockfiles because it is the project's declared intent and carries a
version; a stray lockfile from another manager is common in migrating repositories. When the field
and a lockfile disagree, discovery records both and the `root-package-manager-field` rule may
report the mismatch.

**Pattern expansion** follows pnpm semantics for every manager, because npm, yarn and bun accept the
same pattern shapes and differ only at edges skif does not need to reproduce:

* `dir/*` — direct children of `dir` that contain a `package.json`.
* `dir/**` — `dir` itself if it has a `package.json`, otherwise descend; stop at the first
  `package.json` on each branch. `dir/**/*` excludes `dir` itself.
* `!pattern` — removes matches; negations apply after all positive patterns, regardless of order.
* `node_modules` and dot-directories are never entered.

Discovery returns a `WorkspaceFacts` value holding, among others:

* the root package and each member `Package`, with the manifest path, name, version, `private`,
  `packageManager`, `engines`, and **every dependency entry with its byte span** in the manifest
  (ADR 0007, ADR 0008), so diagnostics point at the exact entry and fixes can edit it;
* `DependencyInstance` values — one per (package, dependency type, name, specifier) — parsed with
  `skif-specifier` (ADR 0006), which is what policies iterate over (ADR 0009);
* catalogs from all sources, with their origin file and spans;
* per-pattern match results, including patterns that matched nothing and directories matched
  without a `package.json`;
* parse failures as data (`ManifestError { path, message, span }`), not as an early exit.

All reads go through the `FileSystem` trait (`read`, `read_dir`, `metadata`), so discovery runs
against an in-memory tree in tests and against a host's buffers when embedded (ADR 0003).
Discovery is parallel per pattern root and its output is sorted by path, so results are
deterministic regardless of thread scheduling.

## Consequences

Positive:

* `unmatched-workspace-patterns` and `missing-package-manifest` (ADR 0007) are ordinary rules over
  facts; they can be configured, ignored and fixed like any other.
* A broken `package.json` in one member is a diagnostic for that member; the rest of the workspace
  is still linted.
* Spans captured once during discovery serve every rule and every fix without re-parsing.

Negative / risks:

* Applying pnpm `**` semantics to npm and yarn workspaces can differ from what those managers
  install in unusual layouts (a package nested inside another package). This is accepted as the
  more predictable rule and is covered by fixtures.
* Detection order is a heuristic. A repository with no `packageManager` field and two lockfiles is
  resolved by lockfile order; the rule that recommends a `packageManager` field exists to make that
  case rare.
* Capturing spans for every dependency entry costs a CST-aware JSON parse instead of
  `serde_json::from_str`; the 50 ms budget on 500 packages (ADR 0001) includes it.

## Alternatives considered

* **A general glob library for workspace patterns:** rejected — standard globbing has no notion of
  stopping at a `package.json`, so `**` would enumerate every nested `node_modules`-free directory
  and treat nested packages as members.
* **Shelling out to `pnpm ls -r --json` / `npm query`:** rejected — needs the package manager
  installed and a populated `node_modules`, and is orders of magnitude slower.
* **Per-manager pattern semantics:** rejected — four implementations of nearly the same thing,
  for differences that appear only in layouts (packages nested in packages) that are themselves
  worth reporting.
