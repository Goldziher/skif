---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# Workspaces, Roots and Package Managers

## Context and Problem Statement

Every rule needs the same facts about a repository: which package manager it uses, which directories are packages,
which dependencies each manifest declares and where, and which catalogs exist. npm, pnpm, Yarn, Bun, Rush and Lerna
declare those facts in different files with different pattern semantics, and repositories often hold a broken member
manifest, a stray lockfile or no root `package.json`.

Many repositories hold several independent projects: nested workspaces, standalone packages, fixture trees. A run from
the top must lint each project once with its own configuration, and must not report code or dependencies one project
uses from another as unused. The same manifests also mean different installs under different managers and settings: a
member's binary reaches every package under one layout and only its dependents under another. How does skif discover
workspaces and roots, combine several roots, and decide what the detected package manager installs?

## Decision Drivers

* Discovery reports facts; rules judge them, so every judgement is configurable and suppressible
* One pattern semantics, whatever the package manager, and spans for every dependency entry captured once
* Each package is linted by exactly one root, and a project gets the same result however a run reaches it
* An answer about installs follows what the detected manager does with the committed files, never user or environment
  settings
* knip's answer is the default where a package-manager detail would otherwise change it

## Considered Options

* Fact-only discovery with pnpm pattern semantics, nested workspaces as roots of their own, crediting across roots, and
  install semantics of the detected manager read from committed files
* Reproduce each package manager's pattern semantics exactly, rejected because the differences are rare edges and one
  semantics keeps every rule uniform
* Lint a multi-root repository as one merged workspace, rejected because each project has its own configuration and a
  failure in one would hide the others
* One npm-like install model for every manager, rejected because it reports binaries the install provides and misses
  ones it does not

## Decision Outcome

Chosen option: "Fact-only discovery with pnpm pattern semantics, nested workspaces as roots of their own, crediting
across roots, and install semantics of the detected manager read from committed files", because it keeps judgement in
rules, makes a recursive run complete and free of duplicate findings without a second analysis, and settles each
package-manager difference from the files the repository commits.

### Facts, not judgements

`skif-workspace` returns facts and never decides that a fact is a problem; rules do
([ADR 0005](0005-rules-diagnostics-and-fixes.md)). All reads go through `FileSystem`
([ADR 0002](0002-crates-library-api-and-upstream-code.md)). Manifests are parsed once each, in parallel, with a span
table, so every dependency entry, field and pattern has its span without a second parse; each dependency instance holds
a specifier parsed once. A member manifest that cannot be read or parsed is a typed fact reported by
`invalid-workspace-file`, and the rest of the workspace is still discovered. Output is sorted, so it does not depend on
scheduling.

### Package-manager detection

The first match wins:

1. `[workspace] package-manager` in the configuration;
2. the root manifest's `packageManager` field, then `devEngines.packageManager`;
3. the manager a workspace declaration names: `rush.json`, then `lerna.json` `npmClient`;
4. a lockfile: `pnpm-lock.yaml`, then `bun.lock` or `bun.lockb`, then `yarn.lock`, then `package-lock.json`;
5. a `pnpm-workspace.yaml`;
6. the files Yarn Berry leaves (`.yarnrc.yml`, `.yarn/releases`);
7. a root `bunfig.toml` with an `[install]` table;
8. otherwise none.

skif never guesses npm. With no manager, every fix whose correctness depends on the manager is withheld. A declared
field beats a lockfile because it is the project's intent and stray lockfiles are common; rules compare the winner with
the lockfiles on disk ([ADR 0006](0006-version-policies-lockfiles-toolchains-and-updates.md)). Yarn is Berry only on
evidence: a `packageManager` naming Yarn 2 or later or, without one, a `.yarnrc.yml` or a Berry `yarn.lock`.

### Workspace declarations and patterns

* A directory declares a workspace with `workspaces` in `package.json`, a `pnpm-workspace.yaml`, a `rush.json`, or a
  `lerna.json` `packages` list that counts only when nothing else declares members, ranked in that order. A
  `pnpm-workspace.yaml` without `packages:` declares pnpm's implicit patterns, and one whose implicit patterns find no
  package besides the root is no workspace.
* A root `package.json` is optional; without one, the root is an empty stand-in that no rule reports on.
* Patterns use pnpm semantics for every manager: `*` matches direct children, `**` descends, segments may mix text and
  wildcards, and brace alternation is supported. Negations apply after all positive patterns. Discovery never enters
  `node_modules` or dot directories and never follows a pattern out of the root. Paths are root-relative with `/`.
* Bun's exceptions: a literal positive pattern names a member no negation removes; a pattern may start with `..` and
  select members outside the root, which count for the pattern but are not loaded; a member without a `name` is
  excluded from analysis and reported by `missing-package-manifest`.
* When nothing declares a workspace, the root package is the only package (single-package mode). A standalone package is
  published and keeps its dependencies in its own manifest, so the rules about a workspace root's privacy and
  dependencies are silent; every other rule runs.

### Roots

* `find_workspace_root` returns the nearest ancestor of an absolute path that declares a workspace, or the project there
  when no ancestor's workspace lists it. It never climbs above the git repository root.
* A directory that declares a workspace is always its own root, also when an outer workspace lists it. The outer
  workspace leaves its directory to it, so each package is linted by exactly one root.
* `-r`, or a start path with no package or workspace at or above it, selects a multi-root run. The walk below the start
  skips `node_modules`, hidden directories, symbolic links, gitignored directories and `[workspace] discover-ignore`
  (fixture directories by default), and is depth-limited, saying so when the limit cut it. `skif::discover_targets` is
  the one root-discovery entry point.
* Each root takes its own layered configuration
  ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)); roots with equal configuration share one
  linter. A root that cannot be loaded is reported in the run's errors with exit code 2 and the others are still
  linted; the exit code is the worst over all roots. Roots are linted in parallel and results keep path order.
* A standalone package with another root above it is nested, and the outer project decides its package manager.

### Crediting across roots

One lint per root; the uses one root sees in another root's territory are credited after every root is linted:

* A dependency listed only above a root is credited to the innermost manifest that lists it, where Node.js finds it, and
  the root that holds that manifest drops its unused-dependency findings for the name.
* A report lists the files of other projects that its code imports, that a script runs or that a plugin names as an
  entry, and separately the files a plugin names as build sources. A listed file below the listing root goes to the
  innermost root that holds it, as an extra entry or build source; a listed file outside the listing root is credited as
  used by the root that holds it.
* A file outside the root is followed but not judged; installed code below `node_modules` is left out, and a use
  outside every root of the run is dropped.
* The lint after `--fix` hands every root the same entries and credits the same uses, so the re-lint judges each root as
  the first lint did. A root linted alone still reports these findings.

### What the package manager installs

* A workspace member's binary reaches a package that does not list the member only when the manager links every
  member's binaries into the root: npm (unless the root `.npmrc` sets `install-strategy=linked`), Yarn Classic, pnpm
  with the `hoisted` node linker, Bun with the hoisted linker, and an unknown manager. Under pnpm's isolated and `pnp`
  linkers, Yarn Berry, Bun's isolated linker, npm's linked strategy and in a Rush repository, it reaches only the
  packages that list the member.
* Linker settings are read from committed files only: for pnpm, `nodeLinker` in `pnpm-workspace.yaml`, else the root
  `.npmrc` only for a pnpm that still reads layout settings there; for Bun, `[install] linker` in the root
  `bunfig.toml`, else what `bun.lock` records. `.npmrc` is read as npm's parser reads it. A member's `.npmrc`, the
  user's `.npmrc`, `npm_config_*` variables and command-line flags are not read.
* An external binary is credited only to a package that, itself or through an ancestor, lists the dependency that
  provides it.
* `[js] use-public-hoisting` (default `false`, top level only) lets packages the install hoists on purpose satisfy an
  import no manifest lists. The pnpm hoist settings decide which, and in a Bazel repository the `public_hoist_packages`
  of the root module do ([ADR 0011](0011-nx-lerna-and-bazel.md)). Hoisting satisfies an unlisted import and makes
  nothing used or unused, and it is ignored under isolation and strict mode, so knip's answer stays the default. A flat
  layout (pnpm's `hoisted` node linker, npm, Yarn Classic, Bun's hoisted linker) is not public hoisting.
* A Rush repository is always isolated, and `rush`, `rushx` and `rush-pnpm` are provided commands there.

### Consequences

* Good, because every judgement is an ordinary rule with a configurable severity, and a broken member manifest still
  yields a report of everything else.
* Good, because `skif .` at the top of a repository of independent projects lints each project once, with its own
  configuration, and a failure in one does not hide the others.
* Good, because dependencies and files another root uses are not reported unused, and `--fix` does not remove them.
* Good, because member binaries, Bun workspaces and lockfiles get the answer the detected manager's install gives.
* Bad, because pnpm's pattern semantics differ from npm and Yarn at rare edges, which skif does not reproduce.
* Bad, because a host that lints roots itself must route the cross-root lists, and a root linted alone cannot see the
  uses of the roots around it.
* Bad, because a linker or hoist setting made only in the user's `.npmrc`, the environment or the command line is not
  seen, and Bun members outside the root are not linted.

### Confirmation

* `crates/skif-workspace/tests/memory.rs` and `crates/skif-workspace/tests/discovery.rs` cover detection precedence,
  pattern semantics, broken members, Rush, Lerna, Bun members and single-package mode.
* `crates/skif/tests/root.rs` covers root finding and the repository ceiling.
* `crates/skif-cli/tests/multi.rs`, `crates/skif-cli/src/roots.rs` and `crates/skif-js/tests/fp_resolve.rs` cover nested
  roots, failing roots, the exit code, the walk limits and crediting across roots, including after `--fix`.
* `crates/skif-js/tests/monorepo_regressions.rs` covers member binaries under each manager and linker, and
  `crates/skif-js/tests/public_hoisting.rs` covers hoisting and its interaction with strict mode.
