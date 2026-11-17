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

Measured, the mode did not pay for itself. Should skif keep it?

## Decision Drivers

* Accuracy of findings: a finding is worth less than the trust a false one costs
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

* Types changed the findings of exactly one rule, `unused-class-members`.
* A typed run cost 1.3x to 16x the untyped run: about knip's speed, and slower than knip on babel and angular.
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

### What stays

* `unused-class-members` stays, matched by name as the untyped run always did
  ([ADR 0009](0009-what-counts-as-a-use.md)).
* The cache stays, with module facts only ([ADR 0007](0007-js-analysis-core-on-oxc.md)).
* Nothing in skif replaces the typed mode. The code moves to the polylint repository, which owns it from here: the
  tsrs-facing layer (program construction, the shared parse registry) as `polylint-tsrs`, and the typed member
  analysis as a second crate. skif publishes neither. The tag `pre-tsrs-removal` marks the last commit that holds
  the typed mode.

### Consequences

* Good, because every run is the fast one, and the findings of a run no longer depend on a mode.
* Good, because skif drops a large pre-release dependency with process-level constraints (an address-space
  reservation, environment variables, a global worker pool, an unsafe call to free programs) and the binary no longer
  carries the checker and its lib files.
* Good, because the class-member findings skif reports on the seeded workspace are all true.
* Bad, because name matching misses an unused class member whose name another class's member shares: on the seeded
  workspace skif finds 15 of 20 unused class members.
* Bad, because configurations, command lines and npm calls that use the removed key, flags and options must drop
  them.

### Confirmation

* The workspace holds no `skif-js-types` crate and `Cargo.lock` no tsrs crate.
* The precision table of the benchmarks page reports skif's class-member precision and recall on the seeded workspace
  ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).
