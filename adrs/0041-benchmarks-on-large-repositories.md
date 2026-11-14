---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Benchmarks on Large Repositories: Capped Run Counts, the Comparison's knip Configuration and knip Errors After a Whole Report

## Context and Problem Statement

The published speed numbers ([ADR 0029](0029-performance-caching-and-benchmarks.md)) time skif against knip on the
`bench` repositories of `corpus.toml`, every command in 15 runs after 3 warmup runs, and knip with the repository's own
configuration. The bench repositories were the five small and medium ones; the largest trees of the corpus (vscode,
next-js, angular and n8n) were compared with knip ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)) but never
timed, so the speed claims say nothing about the scale where speed matters most. Timing them as the others are timed
does not work: knip takes tens of seconds per run on each, so 18 runs of every tool on all four would not fit the
workflow's time limit; n8n's own knip configuration turns off every finding but the dependency ones, and vscode's
workbench is reached only from entry points its bundler script names, so knip with the repository's configuration does
not do the work skif does; and knip exits with an error on next-js and n8n after printing its whole report, because a
tool configuration there does not load, which ADR 0029 publishes as `fails` with no time. How are large repositories
timed against knip?

## Decision Drivers

* Both tools analyse the same files on a timed repository, as the knip comparison makes them do
* A full benchmark run fits the 300-minute limit of one workflow job on a 4-CPU runner
* The small targets keep their run counts, so their medians stay as steady as before
* A published time says how it was measured: run counts, configuration and any error beside it
* One run, one summary and one import publish every number

## Considered Options

* Per-repository caps on the run counts, the comparison's knip configuration, and knip errors after a whole report
  timed with the reason beside them
* Fewer runs for the whole workflow
* A separate workflow for the large repositories
* Declare knip on next-js and n8n as failing, as on create-t3-turbo and vue-core

## Decision Outcome

Chosen option: "Per-repository caps on the run counts, the comparison's knip configuration, and knip errors after a
whole report timed with the reason beside them", because it times the large repositories in the same run as the others
without weakening the other targets, makes both tools do the work the knip comparison checks, and publishes a knip time
wherever knip produced its report.

### Run counts

* `bench-max-runs` and `bench-max-warmup` in `corpus.toml` cap hyperfine's measured and warmup runs on a bench
  repository below a higher `--runs` and `--warmup`; a lower one is kept, so a quick dispatch stays quick. A cap of 0
  measured runs, or a cap outside the bench suite, is rejected.
* vscode, next-js, angular and n8n run 5 times after 1 warmup run. Every target records its `warmup` and `runs` in
  `summary.json` (schema 6); `summary.md`, the provenance sentence and the claims of the published tables name the counts
  of a capped target, and an error count is out of the target's own runs.

### What knip and skif read

* `bench-knip-config = "parity"` times knip with the configuration of the knip comparison in place of the repository's
  own (empty, apart from the corpus-level `entry` files) and skif with a `--config` that adds the same `entry` files,
  written by the same code the comparison writes them with. It is the only value, is rejected outside the bench suite,
  and a bench repository that sets `entry` must set it. The four large repositories set it; the others keep timing knip
  with the repository's own configuration.

### knip errors after a whole report

* A `bench-knip-failure` may set `timed = true`: knip exits with that error after printing its whole report, without
  the entries of a configuration it could not load. A run that matches it is timed and compared with skif like a clean
  run, its check is `pass` or `FAIL` with the reason in its description, and the published tables give its time and
  ratio with the reason. A declaration that no longer matches stays `invalid`, as ADR 0029 decides. next-js (its root
  `jest.config.js` requires the unbuilt `next/jest`) and n8n (two Vite configurations) declare it.
* This replaces, for `timed` declarations only, ADR 0029's rule that a declared knip failure is shown as not measured.

### Disk

* The corpus checkouts keep their installs until every target is timed, so no install runs between two measurements.
  The workflow removes toolchains no step uses (Android, .NET, GHC, CodeQL) before anything else, so the installs of
  the large repositories fit on the runner, and reports the disk usage after the benchmark.

### Consequences

* Good, because the speed claims cover the largest trees of the corpus, with the same tool versions, machine and run.
* Good, because skif and knip read the same files on the large repositories, as the agreement table counts them.
* Good, because the small targets keep 15 runs after 3 warmup runs.
* Bad, because five runs give a less steady median than fifteen on a shared runner.
* Bad, because a `timed` knip time is that of a run without the entries of the configuration it could not load.
* Bad, because the large repositories time knip with a configuration other than the one their maintainers use, so
  their knip times are not those of the repository's own knip setup.
* Bad, because the run takes longer, and the installs of the large repositories need the disk the workflow frees.

### Confirmation

* `tools/bench/src/corpus.rs`: `the_bench_run_caps_and_the_comparison_configuration_are_read_and_checked`,
  `the_large_bench_repositories_run_five_times_with_the_comparison_configuration` and
  `the_bench_repositories_whose_knip_run_fails_declare_the_error`.
* `tools/bench/src/run.rs`: `a_corpus_repository_caps_the_runs_of_its_commands_below_the_runs_asked_for`,
  `the_comparison_configuration_gives_knip_its_config_and_both_tools_the_entry_files`,
  `a_timed_knip_failure_is_checked_like_a_clean_run_with_its_reason`,
  `a_timed_knip_failure_keeps_its_times_and_ratio_in_the_summary` and
  `a_target_with_capped_runs_says_so_and_counts_its_errors_out_of_its_own_runs`.
* `tools/bench/refresh-numbers.test.mjs`: the cases of a knip error declared as after the report and of a target with
  capped runs.

## Pros and Cons of the Options

### Per-repository caps on the run counts, the comparison's knip configuration, and knip errors after a whole report timed with the reason beside them

* Good, because each repository's settings sit in `corpus.toml` beside the reasons for them.
* Good, because the comparison's configuration is reused, not restated.
* Bad, because a target's run counts and knip configuration now depend on the repository, which the tables have to
  say.

### Fewer runs for the whole workflow

* Good, because every target is measured alike.
* Bad, because the targets that take milliseconds would lose the steadiness of fifteen runs to save time only the large
  repositories need.

### A separate workflow for the large repositories

* Good, because the existing run stays as it is.
* Bad, because two runs on two machines would publish numbers that are not comparable with each other, through two
  imports.

### Declare knip on next-js and n8n as failing, as on create-t3-turbo and vue-core

* Good, because no new kind of declaration is needed.
* Bad, because two of the four large repositories would show no knip time, although knip finished its report there.
