---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0009", "0037"]
---

# The Rest of syncpack's Surface: `list` and `json`, Dry-Run Fixes, Instance Filters, `semver-range-only`, `name~version` and `lerna.json`

## Context and Problem Statement

[ADR 0009](0009-version-and-catalog-policies.md) re-implements syncpack's version groups and [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md) completes its dependency types, but a syncpack user still missed part of the command line and the configuration ([#35](https://github.com/Goldziher/skif/issues/35)). There was no way to see the valid instances and their states (`syncpack list`, `syncpack json`), to preview a fix (`fix --dry-run`) or to narrow a run to some dependencies (`--dependencies`, `--dependency-types`, `--specifier-types`, `--source`). `skif migrate` wrote syncpack's `semverRangeOnly` as `ignored`, which drops the range check the group exists for, and it dropped `name~version` custom types. A Lerna monorepo that declares its packages only in `lerna.json` was a single package to skif, while syncpack reads the file.

Which of these does skif take over, in which shape, and which stay out?

## Decision Drivers

* A syncpack user must find the commands and the states they know, under syncpack's names
* What a listing or a preview shows must be what a lint run decides, not a second implementation
* Rule IDs, the default scope of [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md) and the JSON report stay stable
* A change that alters what existing configurations report needs more than parity as a reason

## Considered Options

* Take over the commands, filters, `semverRangeOnly`, `name~version` and `lerna.json` on skif's model, and defer severities, dependency groups, scoped catalogs, `source` and catalog entries as instances
* Take over everything in #35 at once
* Document the gaps only

## Decision Outcome

Chosen option: "Take over the commands, filters, `semverRangeOnly`, `name~version` and `lerna.json` on skif's model, and defer severities, dependency groups, scoped catalogs, `source` and catalog entries as instances", because each of these reuses the existing analysis, while the deferred items change how severity or ownership work and need a design of their own.

**`skif list` and `skif json`.** Both show every instance the version groups and the catalog govern, valid or not, ordered by version group (the configured groups, the default group, then the catalog), dependency name and place. The state comes from the same analysis as the policy rules, with syncpack's valid state names (`IsHighestOrLowestSemver`, `IsIdenticalToPin`, `IsLocalAndValid`, `MatchesSemverGroup`, ...). The run lints the workspace once, and a diagnostic of a policy or catalog rule located on an instance gives it that state, rule and severity, which is how the catalog states arrive. A state whose rule is off or ignored keeps its name and has no severity. skif's own names are `UnusedGroup`, `InvalidPin`, `DiffersToOtherCatalog` and `IsIdenticalToCatalog`. Instances outside every group's scope are not listed. `json` prints one object per instance with syncpack's keys (`dependency`, `dependencyType`, `package`, `versionGroup`, `preferredSemverRange`, `statusCode`, `statusType`, `actual`, `expected`) plus `property`, `packageName`, `versionGroupIndex`, `versionGroupLabel`, `rule`, `severity` and `isLocal`. Both exit `1` when an instance is reported at `error` severity. `list --show` takes syncpack's `instances`, `statuses`, `hints`, `ignored`, `all` and `none`.

**`--dry-run`.** With `--fix` or `--fix-unsafe` it plans the fixes as a fix run does, prints them as a unified diff and writes nothing; no install runs. With `--format json` the `fix` object gains `"dry-run": true` and `changes` (`path`, `kind`, `diff`), which is syncpack's `fix --reporter json` in skif's report. The report and exit code are those of the unchanged tree. A dry run is allowed in CI. `--dry-run` without a fixing flag is a usage error.

**Instance filters.** `--dependencies`, `--dependency-types`, `--specifier-types` and `--source` drop the instances they do not select before groups are assigned, as syncpack does, so a filtered instance is neither reported nor aligned with. A non-empty `--dependency-types` is also the default scope of the run, so `--dependency-types peer` governs peer ranges. `--source` matches the file an instance is declared in and does not change which packages form the workspace. The filters reach the version-group and semver-group rules and the listing; the catalog, workspace-protocol, family and manifest rules see the whole workspace. A configured group the filters leave empty is not reported as `UnusedGroup`. The filter is a field of `Config` that no file can set.

**`semver-range-only`.** A version-group policy, syncpack's `semverRangeOnly`: versions may differ, an instance whose range is not the one its semver group asks for is `SemverRangeMismatch` with the corrected range as the fix, and every other instance is `MatchesSemverGroup` (a package's own version is `IsLocalAndValid`). Registry updates inside the group stay out of scope ([ADR 0001](0001-mission-and-scope.md)). `skif migrate` writes it instead of `ignored`. This adds a policy to the list of ADR 0009.

**`name~version`.** A custom type strategy with a `name-path`: the dependency name is the string at `name-path`, the version the string at `path`, and a fix rewrites the version. The instance is located at the nearest member that holds both fields. `skif migrate` translates it from syncpack's `namePath`. This replaces the sentence of ADR 0037 that the strategy is not supported; `source` stays unsupported.

**`lerna.json`.** When nothing else declares members, the `packages` of a `lerna.json` next to the root `package.json` do, after `package.json`, `pnpm-workspace.yaml` and `rush.json`, and its `npmClient` names the package manager after `rush.json`. A directory with such a file is a workspace root.

**Deferred.** Per-group and per-state `severity` (severity stays per rule, [ADR 0007](0007-rule-and-diagnostic-model.md)), `dependencyGroups`, a `catalog` policy scoped by selectors, custom type `source`, catalog entries as instances, `.x` ranges and `--log-levels`. syncpack 15.3.3 classifies `1.x` as unsupported too (`Specifier::create` in `crates/syncpack-specifier`), so reading it as a range would be a deliberate deviation from the vendored model ([ADR 0006](0006-specifier-and-semver-model.md)). skif writes no log, so `--log-levels` has nothing to control.

### Consequences

* Good, because a syncpack user can list states, preview fixes and narrow a run as before, and the listing cannot drift from the rules.
* Good, because a migrated `semverRangeOnly` or `name~version` keeps its meaning.
* Bad, because the filters do not reach the catalog rules, so `--dependencies react` still reports catalog findings for other dependencies.
* Bad, because `--source` filters instances where syncpack replaces discovery; a package outside the workspace cannot be added with it.
* Bad, because `IsIdenticalToCatalog` is a state name syncpack does not have.

### Confirmation

* Unit tests in `crates/skif-rules/src/policy/inspect.rs` cover group variants, valid and invalid states, the catalog states and the filters.
* `fixtures/policy/semver-range-only` (3 cases) and `fixtures/policy/dependency-types/custom-type-name-tilde-version` run in the policy conformance suite.
* `crates/skif-cli/tests/cli.rs` covers `list`, `json`, `--dry-run` and the filters, with snapshots.
* `crates/skif-workspace` tests cover `lerna.json` discovery and root claims; `crates/skif-migrate/tests/golden.rs` covers both translations.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Take over the commands, filters, `semverRangeOnly`, `name~version` and `lerna.json` on skif's model, and defer severities, dependency groups, scoped catalogs, `source` and catalog entries as instances

* Good, because every addition reuses the analysis, the fix planner or discovery.
* Bad, because part of #35 stays open.

### Take over everything in #35 at once

* Good, because the syncpack surface would be complete.
* Bad, because per-state severity contradicts ADR 0007, and dependency groups and catalog entries as instances change which instances exist for every rule.

### Document the gaps only

* Good, because it needs no code.
* Bad, because `semverRangeOnly` keeps losing its check on migration.

## More Information

* Amends: [ADR 0009](0009-version-and-catalog-policies.md) (the list of policies), [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md) (the `name~version` strategy)
* Related: [ADR 0005](0005-workspace-discovery.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md)
* See the [ADR index](index.md).
