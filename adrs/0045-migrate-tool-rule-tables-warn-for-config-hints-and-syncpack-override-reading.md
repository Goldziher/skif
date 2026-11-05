---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# `skif migrate`: Rule Coverage Tables, `warn` for Configuration Hints and Syncpack's Reading of Overrides

## Context and Problem Statement

[ADR 0041](0041-skif-migrate-composable-steps-full-validation-and-in-place-merge.md) made the `exact` profile turn off every default rule the migrated tools did not run. A migration of a real repository, whose three tools all passed, still failed under the generated configuration for three reasons.

First, which rules a tool "runs" was a hand-kept list per translator: syncpack kept eight rules whatever its configuration, and knip kept `unparseable-files`. Reading the sources shows both are wrong. syncpack 15 reports catalog states only for a version group with `policy: catalog` (`NotUsingCatalog`, `MissingFromCatalog`, `DependsOnMissingCatalogDefinition`); it reports `DiffersToCatalog` in every highest or lowest group. knip skips a file it cannot parse without reporting it.

Second, knip's `treatConfigHintsAsErrors` was written as `stale-js-config = "error"`. The hints of skif differ from those of knip: skif also flags entries it never needed (a regular expression in `ignoreUnresolved` for an import skif does not report), so the project failed where knip passed.

Third, a clean syncpack run produced `version-mismatch` errors for override entries only. syncpack 15 reads `pnpmOverrides` from the `overrides` of `pnpm-workspace.yaml` and `overrides`, `resolutions` from `package.json`; it takes the key of an entry as the dependency name and ignores nested objects; it does not read `pnpm.overrides` of a manifest. skif names the package a selector replaces (`parent@4>child` is `child`), reads nested objects, and reads `pnpm.overrides` of every manifest, so entries syncpack never compared with `child` were compared with it.

## Decision Drivers

* A project that passed the old tools must pass the migrated configuration in the `exact` profile
* The set of rules a tool runs should come from data that a test checks against the rule list, not from code
* Defaults of skif that are deliberate should stay; only a migrated configuration opts into syncpack's reading

## Considered Options

* Per-tool coverage tables as data, a `warn` level for configuration hints behind `--strict-hints`, and a `[policy] override-reading` key
* Keep the lists in code and add the missing cases
* Change the default reading of overrides in skif to syncpack's

## Decision Outcome

Chosen option: "Per-tool coverage tables as data, a `warn` level for configuration hints behind `--strict-hints`, and a `[policy] override-reading` key", because it fixes the three failures without moving a default.

**Coverage tables.** `rule-renames.txt` (sherif), `knip-types.txt` (knip) and the new `tool-rules.txt` (syncpack, with the condition under which it reports the rule, and the rules no migrated tool runs, with a reason) are the only places that say which rule a tool runs. The translators read them. A test requires every rule of `rule-ids.txt` to be in exactly one of the tables or in the `none` rows, so a new rule must be classified. syncpack's `catalog-not-used`, `catalog-missing-entry` and `unresolved-catalog-references` are kept only when a `policy: catalog` group is written to `[catalog] mode`; `unparseable-files` and `stale-js-config` are no longer kept for knip.

**Configuration hints.** `treatConfigHintsAsErrors` and `--treat-config-hints-as-errors` write `stale-js-config = "warn"` with an approximate note. `skif migrate --strict-hints` writes `"error"`. `--no-config-hints` still writes `"off"`.

**Override reading.** `[policy] override-reading` is `"skif"` (default) or `"syncpack"`. With `"syncpack"` the key of an override entry is the dependency name, nested objects are skipped and `pnpm.overrides` of a manifest is not read; `overrides` of `pnpm-workspace.yaml` stays. The migration of syncpack writes `"syncpack"`.

### Consequences

* Good, because the profile for a tool is derived from tables that tests check against the rule list.
* Good, because a clean syncpack run stays clean for override entries.
* Good, because `--strict-hints` keeps the old behaviour available.
* Bad, because a user of the `syncpack` reading does not get skif's finding that an override of `child` differs from the dependency on `child`.
* Bad, because `catalog-mismatch` stays on for syncpack in every group, although syncpack reports it only in highest or lowest groups.

### Confirmation

* Golden tests assert the `off` rules per tool, the catalog rules with and without `policy: catalog`, `warn` and `error` with `--strict-hints`, and the `[policy]` key.
* A unit test collects the instances of a workspace with both readings.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Per-tool coverage tables as data, a `warn` level for configuration hints behind `--strict-hints`, and a `[policy] override-reading` key

* Good, because each rule has one documented owner.
* Bad, because the key is a second model of what an override entry is.

### Keep the lists in code and add the missing cases

* Good, because it is a small change.
* Bad, because the next rule is missed again without a test noticing.

### Change the default reading of overrides in skif to syncpack's

* Good, because there is one model.
* Bad, because [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md) chose the package a selector replaces on purpose, and the change would hide real inconsistencies for users who did not come from syncpack.

## More Information

* Supersedes the paragraph "Override sections" of [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md) where it says the dependency of an entry is always the package a selector replaces, and the knip statement of [ADR 0041](0041-skif-migrate-composable-steps-full-validation-and-in-place-merge.md) that `unparseable-files` belongs to every knip run.
* Sources: syncpack 15.3.3 `src/rcfile.rs` (default dependency types), `src/sources.rs` (descriptors), `src/version_group/catalog.rs`; knip 6 `ProjectPrincipal.ts` (parse errors are skipped); sherif 1.13 `src/rules`.
