---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Performance Budget and Benchmarks With Types

## Context and Problem Statement

[ADR 0029](0029-performance-caching-and-benchmarks.md) makes speed a published property: numbers come from the pinned
benchmark runner, never from a laptop. Type awareness ([ADR 0044](0044-type-aware-analysis-on-tsrs.md)) is on by
default and builds TypeScript programs beside the oxc graph, so the default run gets slower. Users choose between the
two modes, and they need the numbers to choose.

What may type awareness cost, how is it measured, and what does skif publish?

## Decision Drivers

* The default mode's cost is published beside the untyped mode's, from the same runner
* A budget that a change must stay within, checked in the benchmark workflow
* skif stays much faster than knip with types on

## Considered Options

* A budget per mode with both columns in the benchmark workflow and the published tables
* Publish only the default mode
* No budget; publish whatever the workflow measures

## Decision Outcome

Chosen option: "A budget per mode with both columns in the benchmark workflow and the published tables", because users choose the mode from published numbers and a budget keeps the default from
drifting slower.

### Budget

* Types off: no cost against the release before type awareness; nothing of tsrs runs.
* Types on, measured on the benchmark runner: at most 3× the untyped run on the largest repository of the corpus, a
  median of at most 2× across the corpus, at most 3 GiB peak RSS, at most 5 s on the generated 10,000-file repository,
  and still at least 5× faster than knip wherever knip completes.
* A change that breaks the budget is not merged without a decision recorded in an ADR.

### Measurement

* The benchmark harness times both modes per repository; the workflow summary and the published tables show both
  columns, labelled "types on (default)" and "types off".
* The synthetic generator emits per-package `tsconfig.json` files and exported classes and enums, so the generated
  repositories exercise the checker.
* `SKIF_TIMINGS` prints the type-aware stages (programs, queries) per root.

### Consequences

* Good, because users see the trade-off in numbers.
* Bad, because the benchmark workflow runs skif twice per repository and takes longer.

### Confirmation

* The benchmark summary schema has both modes; `refresh-numbers` fails when a published table lacks one.
* The workflow fails a run whose typed column breaks the budget against its untyped column.

## Pros and Cons of the Options

### Both modes with a budget

* Good, because the cost is visible and bounded.
* Bad, because more benchmark time.

### Only the default mode

* Good, because less benchmark time.
* Bad, because the switch's effect is unknown.

### No budget

* Good, because nothing blocks a change.
* Bad, because the default run could drift slower unnoticed.
