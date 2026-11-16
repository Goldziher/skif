---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0021", "0029"]
---

# Time Each Comparison Tool on the Scope It Reads

## Context and Problem Statement

`skif-bench run` times skif against sherif, syncpack and knip ([ADR 0029](0029-performance-caching-benchmarks-and-budgets.md)),
and the published speed claims are ratios of those times. On the corpus repositories that set `recursive` in
`corpus.toml` (babel, vscode, next-js), the harness ran every skif row with `--recursive`, as `parity` does
([ADR 0021](0021-knip-and-syncpack-comparison-harness-and-its-known-deviations.md)). A recursive run lints every nested
root that is not a workspace member: next-js has 403 of them. sherif reads the root workspace's `package.json` and
`pnpm-workspace.yaml`, syncpack reads the workspace packages, and knip reads one workspace. So the compared tools did
less work than skif, and the ratios of those three repositories compared different amounts of work.

Which scope is skif timed on when a ratio or a check compares it with another tool?

## Decision Drivers

* A ratio is only as meaningful as the equality of the work behind it
* The agreement measurement of ADR 0021 still needs the recursive run, because it compares findings, not time
* The cost of a recursive run is worth publishing, but not as a comparison with a tool that does not do that work
* The ADR 0001 targets and the type-aware budget are stated against the tools' own scope

## Considered Options

* Time each compared row on the scope the comparison tool reads, and time a recursive run as an informational row
* Keep timing skif recursively where the repository sets `recursive`
* Time the comparison tools on every nested root too, by running each in a loop over the roots

## Decision Outcome

Chosen option: "Time each compared row on the scope the comparison tool reads, and time a recursive run as an
informational row", because it is the only option in which every published ratio divides the time of two runs that
read the same projects, and it keeps the cost of a recursive run visible.

* `skif --no-js`, compared with sherif and syncpack, is timed without `--recursive`: the root workspace is the scope
  both read.
* The full `skif` row, compared with knip and checked by ADR 0001 and the type-aware budget, is timed on the scope knip
  reads. The rule: without `--recursive`, unless the repository configures knip itself to read nested projects that
  are not workspace members. The benchmark times knip with the comparison's configuration (`bench-knip-config =
  "parity"`: an empty configuration plus the repository's `entry` files) or with the repository's own, and none of
  babel, vscode and next-js configures knip with workspaces beyond the root's, so all three are timed without
  `--recursive`. A repository that did configure knip that way would time its compared row with `--recursive`; none
  does, and the harness has no key for it until one needs it.
* `skif --type-aware` is timed on the scope of the full `skif` row, so its ratio to that row stays one of equal scope.
* Where `recursive` is set, the harness also times `skif --recursive`, a full recursive run, as an informational row.
  It has no ratio and no peak RSS, is in no ADR 0001 or budget check, and enters no published table or claim except
  the detail table of its target, which says so. `recursive` keeps its meaning for `parity`.
* `SUMMARY_SCHEMA` goes from 7 to 8, so a summary measured before this record, whose ratios compare unequal scopes, is
  refused by `refresh-numbers.mjs`.

### Consequences

* Good, because every published ratio compares runs of equal scope.
* Good, because the cost of a recursive run stays published beside the compared rows, not hidden in them.
* Bad, because the published ratios of babel, vscode and next-js change, and the speed claims drawn from them with
  them. The results in `tools/bench/results` are the last run's, at schema 7; the next dispatched run replaces them
  through `mise run bench:import`, and until then they are refused by `refresh-numbers.mjs`.
* Bad, because a recursive run adds one timed command on each of those repositories, which on next-js is the longest.
* Neutral, because `parity`, its baselines and the agreement table are unchanged: they compare findings, not time.

### Confirmation

* `tools/bench/src/run.rs`:
  `a_recursive_corpus_repository_is_compared_without_recursive_and_timed_recursively_beside_them` and
  `the_recursive_row_has_no_ratio_and_no_check`, and
  `the_comparison_configuration_gives_knip_its_config_and_both_tools_the_entry_files`, which checks that the skif
  arguments of a recursive repository hold no `--recursive`.
* `tools/bench/refresh-numbers.test.mjs`: "an informational recursive row prints no ratio, is no required row and
  joins no table or claim" and the old-schema case of "a skipped tool, a missing row or an old schema is a problem".

## Pros and Cons of the Options

### Time each compared row on the scope the comparison tool reads, and time a recursive run as an informational row

* Good, because ratios compare equal work and the recursive cost is still published.
* Bad, because it changes published numbers and adds a timed command.

### Keep timing skif recursively where the repository sets `recursive`

* Good, because skif's row covers every project the repository holds.
* Bad, because the ratio divides the time of a run that lints hundreds of projects by that of a tool that reads one.

### Time the comparison tools on every nested root too, by running each in a loop over the roots

* Good, because every tool would read every project.
* Bad, because it times a harness loop, not a tool, and sherif and syncpack are not designed to be run per nested
  project.

## More Information

This amends ADR 0029 (what the benchmark times and compares) and ADR 0021 (the benchmark no longer runs skif as
`parity` does where `recursive` is set; `parity` itself is unchanged). The rest of both records holds.
