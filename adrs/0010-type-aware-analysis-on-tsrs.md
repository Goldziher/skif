---
status: accepted
date: 2026-11-17
decision-makers: ["Na'aman Hirschfeld"]
---

# No Type-Aware Analysis

## Context and Problem Statement

skif's JS analysis has no type checker ([ADR 0007](0007-js-analysis-core-on-oxc.md)). `unused-class-members`
([ADR 0009](0009-what-counts-as-a-use.md)) therefore credits a member when any module reads a property of its name, so
a read of `dispose` on one class keeps every `dispose` in the project used.

To settle such reads, skif had an opt-in typed mode (`--type-aware`, `[js] type-aware`, the npm option `typeAware`)
built on tsrs, a pre-release Rust port of the TypeScript 7 compiler and its checker, in the crate `skif-js-types`. It
built a program per `tsconfig.json`, asked the checker which declarations each member read lands on, fell back to
names where types were missing, and was held to a published time and memory budget.

Should skif keep the mode, given its measured cost and accuracy?

## Decision Drivers

* Precision before recall: a false finding costs more trust than a missed one
* Speed is a product property ([ADR 0001](0001-mission-scope-and-non-goals.md))
* A dependency, a mode and a set of flags are kept only for findings they improve

## Considered Options

* Remove type-aware analysis, tsrs and `skif-js-types` from skif
* Keep it opt-in, as it was
* Make it the default once it meets its budget, as first planned

## Decision Outcome

Chosen option: "Remove type-aware analysis, tsrs and `skif-js-types` from skif", because the measurements below show
a mode that changes one rule, costs up to 16 times the run without it and is less precise where it differs.

### What was measured

Speed is from the release bench of skif 0.7.0. Precision is from skif 0.8.0 (commit `4d3bdd7f`) against knip 6.45.1
on 73 seeded findings, measured 2026-11-17. The typed mode is in release `v0.8.0`, the last one with it.

* Types changed the findings of exactly one rule, `unused-class-members`.
* A typed run cost 1.3x to 16x the untyped run. That is between 0.5x and 5.3x knip's speed on the seven repositories
  where knip completes: half as fast on babel, level on angular.
* On the seeded dead-code workspace ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)) the typed run was less
  precise: 81% precision against 100% without types. It reported live class members read through an interface
  satisfied without `implements`, through a generic constraint, and through a key of literal type.
* Its only gain there was recall, 86% against 79%, on same-named members of different classes.

### What is removed

* The flags `--type-aware` and `--no-type-aware`, the key `[js] type-aware` at the top level and per workspace, the npm
  options `typeAware` and `noTypeAware`, and the `type-aware` object of the report
  ([ADR 0012](0012-cli-contract-and-reporting.md)).
* The tsrs dependency, the crate `skif-js-types` and the facade features `type-aware` and `plain-ptrs`
  ([ADR 0002](0002-crates-library-api-and-upstream-code.md)).
* The planned typed cache layers, the type-aware budget and the typed benchmark rows
  ([ADR 0007](0007-js-analysis-core-on-oxc.md), [ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).
* Removed surface is rejected, never ignored, and the message names the removal in 0.9.0. A removed flag is a usage
  error. A removed key (`type-aware` under `[js]`, `[js.workspaces."<dir>"]` or the `js` table of an `[[overrides]]`
  entry) is a configuration error that names the file, line and column. Both exit `2`
  ([ADR 0012](0012-cli-contract-and-reporting.md)). The npm package rejects the options `typeAware` and
  `noTypeAware` with the same message.

### What stays

* `unused-class-members` stays, matched by name as the untyped run always did
  ([ADR 0009](0009-what-counts-as-a-use.md)).
* The cache stays, with module facts only ([ADR 0007](0007-js-analysis-core-on-oxc.md)).
* Nothing in skif replaces the typed mode. The code is being moved to the polylint repository, which owns it from
  there: the tsrs-facing layer (program construction, the shared parse registry) as `polylint-tsrs`, and the typed
  member analysis as a second crate. skif publishes neither. Release `v0.8.0` is the last with the typed mode and
  holds its code.

### Consequences

* Good, because no run pays the 1.3x to 16x of a typed run, and findings no longer depend on a mode.
* Good, because skif drops a large pre-release dependency with process-level constraints (an address-space
  reservation, environment variables, a global worker pool, an unsafe call to free programs) and the binary no longer
  carries the checker and its lib files.
* Good, because the class-member findings skif reports on the seeded workspace are all true.
* Bad, because name matching misses an unused class member whose name is read on any other object: on the seeded
  workspace skif finds 15 of 20 unused class members.
* Bad, because configurations, command lines and npm calls that use the removed key, flags and options must drop
  them.

### Confirmation

* The workspace holds no `skif-js-types` crate and `Cargo.lock` no tsrs crate.
* `the_removed_type_aware_flags_say_they_were_removed` and `a_removed_type_aware_key_in_skif_toml_says_it_was_removed`
  in `crates/skif-cli/tests/cli.rs` assert the exit code, the error kind and the message of a removed flag and key.
* `the_removed_type_aware_key_says_it_was_removed` in `crates/skif-config/tests/config.rs` covers the key in each of
  its three tables, and `the_removed_type_aware_key_fails_in_a_layered_file_with_its_position` in
  `crates/skif-config/tests/layers.rs` its file, line and column.
* The precision table of the benchmarks page reports skif's class-member precision and recall on the seeded workspace
  ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).
