---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Knip Comparison Against the Typed Run

## Context and Problem Statement

[ADR 0046](0046-typed-member-semantics.md) has the comparison harness ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md))
run skif twice per repository, with types and with `--no-type-aware`, set the findings only one mode reports apart as
the `type-resolved` deviation, and count the untyped run in the comparison with knip. Types are on by default
([ADR 0044](0044-type-aware-analysis-on-tsrs.md)), so the counted run is not the one users get.

Which of the two runs does the knip comparison count?

## Decision Drivers

* The published agreement with knip describes skif as it runs by default
* Findings that change with the mode are named, not silent ([ADR 0046](0046-typed-member-semantics.md))
* The counts stay comparable with those recorded before types: a mode change shows only as `type-resolved`
* The second run can be left out where its time matters, without a baseline recorded that way

## Considered Options

* Count the typed run; set apart what the untyped run settles the other way
* Count the untyped run; set apart what the typed run settles the other way (ADR 0046)

## Decision Outcome

Chosen option: "Count the typed run; set apart what the untyped run settles the other way", because the agreement
numbers then describe the default, while the `type-resolved` deviation keeps every difference that types make out of
the counts.

* The knip comparison counts skif's default run, with types. A one-sided finding no other deviation explains is
  `type-resolved` when skif reports it only with types, or when knip reports it and only skif without types does too.
  The other deviations are classified first.
* The baseline header records the mode skif ran in (`mode = "type-aware"`); a baseline of another mode is not
  comparable and is re-recorded. A baseline without a mode predates the field and is compared.
* `tools/bench/baselines/type-aware.json` records per repository and rule the findings of both runs and those only
  one has, counts only, and `--check` checks it with the same tolerance.
* `--no-untyped-pass` skips the second run: `type-resolved` is then not classified and `type-aware.json` not checked,
  and neither baseline can be recorded.

This amends the comparison part of [ADR 0046](0046-typed-member-semantics.md); its credit semantics stand.

### Consequences

* Good, because the agreement table is the agreement of the default.
* Good, because a counted one-sided finding is one both modes agree on, so the counts move only with the analysis,
  not with the mode.
* Bad, because a finding that knip and the untyped run share and the typed run drops leaves the `both` count.
* Bad, because a check run with `--no-untyped-pass` counts `type-resolved` findings as one-sided and may report drift.

### Confirmation

* Unit tests of the harness: the `type-resolved` classification of each side, its order after the other deviations,
  the mode check of the baseline header, and the per-rule counts of `type-aware.json`.
* `mise run parity -- --check` shows drift only in the `type-resolved` category.

## Pros and Cons of the Options

### Count the typed run

* Good, because it measures the default.
* Bad, because the `both` count drops where types remove a finding knip has.

### Count the untyped run

* Good, because it matches knip's model, which has no type checker.
* Bad, because the published agreement is not that of the default and says nothing about typed findings.
