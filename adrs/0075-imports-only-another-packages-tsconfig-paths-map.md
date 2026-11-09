---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
---

# Imports Only Another Package's tsconfig `paths` Map: Unlisted, With a Detail, Not Resolved Through Merged Paths

## Context and Problem Statement

knip resolves every import of a workspace with the tsconfig `paths` of all workspaces merged into one set. A package
`lib` without a `paths` mapping can import `shared/value` because `packages/app/tsconfig.json` maps `shared/*` to
`../lib/src/*`; knip reports nothing. skif resolves an import with the `paths` of the configuration the importing file
follows, as TypeScript, editors and bundlers do (GitHub issue #112). For skif the import names the package `shared`,
which no manifest lists, so `unlisted-dependencies` reports it and advises adding `shared` to `package.json`: advice
that is wrong, since no such package exists and the fix is a mapping or an import by package name.

How should skif report an import that only another package's tsconfig `paths` map, and how is the difference from knip
recorded where users and the comparison meet it?

## Decision Drivers

* skif resolves imports the way the compiler and the bundler of the importing package do; a build of `lib` alone fails
  on such an import
* The advice of a finding must name the fix that applies
* Rule ids are stable, and configurations that set `unlisted-dependencies` must keep covering these imports
* A user who migrates from knip must learn about the difference before the first run, not from a surprise
* The knip comparison must not count a deliberate difference as unexplained disagreement

## Considered Options

* Report the import under `unlisted-dependencies` with the detail `ForeignTsconfigPaths` and its own message
* Merge the `paths` of all packages, as knip does
* Add a rule id for imports that need a mapping

## Decision Outcome

Chosen option: "Report the import under `unlisted-dependencies` with the detail `ForeignTsconfigPaths` and its own message", because it keeps skif's resolution faithful to how the package builds, gives the advice that fixes it, and changes no rule id or configuration.

**Which imports.** An unlisted import gets the detail when all of these hold: it did not resolve to a module of the
graph (it is taken for a package, or nothing was found); its package name is not a workspace package and no manifest
of the workspace, or above it, lists it; no `node_modules` directory at or above the importing file, up to the workspace root, holds it; the
configuration the importing file follows maps it not; and a `paths` pattern of the configuration of another package,
or of a root configuration the package does not follow, matches it with a target that resolves to a source file of the
workspace outside `node_modules`. Patterns rank as TypeScript ranks them: an exact pattern first, then the wildcard with
the longest prefix, then the first configuration by path. A wildcard with no literal prefix (`*`, `*.svg`) matches
every specifier and is not counted. Every other unlisted import keeps the `package.json` advice.

**The finding.** The message says that the specifier resolves only through the `paths` of the named configuration and
to add the mapping or import by package name; the help names the pattern; a related location points at the
configuration file that declares the pattern, which is a configuration the package's own one extends when `paths` are
inherited. There is no fix.

**Migration.** `skif migrate --from knip` cannot see tsconfig files: the library does no I/O and reads only the
configuration it is given. When the knip configuration declares `workspaces`, or the caller passes workspace patterns,
the migration adds an approximate note, at the path `tsconfig paths`: "knip merges the tsconfig paths of all
workspaces; skif does not, so an import that only another package's paths map is reported as unlisted-dependencies
with detail ForeignTsconfigPaths".

**Comparison.** In the knip parity harness, a finding only skif reports in `unlisted` that carries the detail is the
`foreign-tsconfig-paths` known deviation, beside those of
[ADR 0068](0068-what-the-knip-comparison-sets-apart.md): counted in its own column of the known-deviations section,
recorded in the baseline and ignored by `--check`.

### Consequences

* Good, because the advice of these findings names the fix, and the finding links the configuration to copy from.
* Good, because rule configuration, ignores and severities that cover `unlisted-dependencies` keep covering them; a
  consumer that wants them apart reads the detail.
* Good, because the difference from knip is stated in the migration report and kept apart in the comparison.
* Bad, because a project that relies on knip's merged `paths` sees new errors after switching; they are not muted by
  default.
* Bad, because the migration note appears in every multi-package workspace, also where no tsconfig sets `paths`.
* Bad, because `UnlistedImport` gains fields and becomes `#[non_exhaustive]`, a breaking change of the `skif-js` API.

### Confirmation

* `skif-js` integration tests in `crates/skif-js/tests/foreign_tsconfig_paths.rs`, including the cases that keep the
  `package.json` advice: a workspace package, a package another manifest lists, an installed package, a pattern into
  `node_modules` and a pattern whose targets are missing.
* `skif-migrate` golden tests `knip_in_a_workspace_notes_that_skif_does_not_merge_tsconfig_paths`,
  `knip_workspaces_in_the_config_alone_note_the_unmerged_tsconfig_paths` and
  `knip_in_a_single_package_project_says_nothing_about_tsconfig_paths`.
* `skif-bench` unit test `unlisted_imports_only_foreign_tsconfig_paths_map_split_off`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Report the import under `unlisted-dependencies` with the detail `ForeignTsconfigPaths` and its own message

* Good, because the import is still reported: the package does not build on its own.
* Good, because no rule id or configuration key changes.
* Bad, because tools that read only the rule id cannot tell these findings from missing dependencies.

### Merge the `paths` of all packages, as knip does

* Good, because skif and knip would agree.
* Bad, because skif would accept imports that the compiler and the bundler of the importing package reject.
* Bad, because a pattern of one package could hide a real missing dependency of another.

### Add a rule id for imports that need a mapping

* Good, because the finding could have its own severity.
* Bad, because rule ids are stable: configurations that turn `unlisted-dependencies` off or ignore its findings would
  stop covering these imports, and the migration from knip would need a second id for one knip issue type.

## More Information

* Related decisions: [ADR 0010](0010-js-analysis-on-oxc.md),
  [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md),
  [ADR 0068](0068-what-the-knip-comparison-sets-apart.md)
* See the [ADR index](index.md).
