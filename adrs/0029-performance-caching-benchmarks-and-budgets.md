---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Performance, Caching, Benchmarks and Budgets: A Content-Keyed Module Cache, Published Numbers From a Pinned Runner and a Type-Aware Budget

## Context and Problem Statement

Speed is part of skif's mission ([ADR 0001](0001-mission-scope-and-non-goals.md)). Most of an untyped JS analysis run
is oxc's parse and semantic pass of every source file; resolving imports, building the graph and running the rules
cost far less. Long-lived hosts (watch mode, the language server, the MCP server) parse the same unchanged files on
every run, and a multi-root run parses code its roots share once per root. Speed claims also need numbers someone
else can repeat: a figure measured on an unrecorded machine says nothing.

Type-aware analysis ([ADR 0034](0034-type-aware-analysis-on-tsrs.md)) is on by default and adds the type checker's
programs to a run. Its cost is in time and in memory: type-checking a program needs memory in proportion to the
program, and the largest program of the corpus holds most of a typed run's peak, which is still well below what
`tsc --noEmit` needs for the same program. Users choose the mode from published numbers, and without a budget the
default drifts slower. The largest repositories of the corpus also need more time, disk and care than a fixed run
count gives them.

What does skif cache, how do the roots of one run share parse results, where are published speed numbers measured,
and what may a type-aware run cost?

## Decision Drivers

* A cache never changes output: no cache, a cold cache and a warm cache give the same report
* Invalidation is correct by construction, not by tracking which inputs a result depended on
* The library stays I/O-free ([ADR 0003](0003-library-api-and-embedding.md)); persisting a cache is the CLI's job
* A run whose roots share no code, and every single-root run, pays nothing for sharing
* A host's `ModuleCache` stays two methods
* Anyone can repeat a published measurement from the repository, and nothing timed or compared is unpinned
* The repository's commits stay signed by the maintainer: no workflow commits
* The cost of types is published for both modes and bounded by a budget the workflow checks
* The memory budget bounds what skif adds, not what type-checking a program inherently needs, and is checkable without
  running another tool

## Considered Options

* Cache each file's parse result keyed by its content, share a run cache of non-local files with a parse coalescer,
  publish numbers of both modes from a manually dispatched workflow on a pinned GitHub-hosted runner, and fail that
  workflow on a broken type-aware budget
* Cache the whole report, keyed by the inputs of the run
* Cache parse results keyed by path, size and modification time
* Publish numbers measured on the maintainer's machine
* A scheduled benchmark workflow that commits its results
* A memory budget relative to `tsc`'s peak on the largest program, measured in the workflow

## Decision Outcome

Chosen option: "Cache each file's parse result keyed by its content, share a run cache of non-local files with a
parse coalescer, publish numbers of both modes from a manually dispatched workflow on a pinned GitHub-hosted runner,
and fail that workflow on a broken type-aware budget", because a parse result is a pure function of the file's name,
text and parse options, so a key over those can never be stale, a dispatched workflow on a pinned image is the only
measurement anyone can repeat with nothing but the repository, and a checked budget keeps the default's cost visible
and bounded.

### The cache unit and its key

* The unit is the `ModuleInfo` that parsing one file returns. Everything else (configuration, manifests, lockfiles,
  the git index, plugin configs, resolution, type facts, the graph and the rules) is computed on every run.
* `ModuleKey` is a 128-bit SipHash-1-3 over `CACHE_SCHEMA` and the `skif-js-syntax` version, the parser kind, the
  parse flags, the file name (which selects the language) and the text with its length. The directory is not part of
  the key, so identical files share an entry and a moved checkout keeps its cache.
* `CACHE_SCHEMA` is raised whenever parse output changes; a test parses a probe corpus and compares a digest of the
  results with a constant beside the schema, so a change to the parser fails until both are updated.
* A failed lookup or an entry that does not decode is a miss, never an error.

### The library boundary

* `skif-js-syntax` owns `ModuleKey`, `CachedModule` (an opaque serializable wrapper, so `ModuleInfo` is no public
  contract), `trait ModuleCache` with `get` and `put`, `MemoryModuleCache` (pruned to the entries the last run used),
  `parse_cached` and `ParseCoalescer`. The facade re-exports them.
* The cache is given to the linter, not to the run options: `Linter::with_module_cache`, and for several targets
  `Linter::shared_modules` and `Linter::for_targets`.

### Sharing within one run

`SharedModules::for_run(roots, host)` holds the whole policy:

* a host cache and one root: the host cache for every file, no coalescing;
* a host cache and several roots: the host cache for every file, coalesced;
* no host cache and several roots: an in-memory run cache, coalesced, holding only files that are not local to the
  root parsing them (a file is local when it lies below that root and below no root nested in it), so a run whose
  roots share no code hashes, stores and copies nothing;
* otherwise no cache.

`ParseCoalescer` sits beside the cache, not inside it, so a wrapping cache cannot drop the coordination: concurrent
lookups of one key wait for one parse, each key has its own condition variable, a waiter is handed a parse error as
well as a result, and a panicking parse releases its claim to the next waiter. A parse never waits on another lookup
through the coalescer, which is what makes it deadlock-free.

### The CLI cache

* `--cache` is opt-in and has no configuration key, because a cache is not a setting of the project.
  `--cache-location` requires `--cache`. The default store is `node_modules/.cache/skif/modules.bin` below the
  directory the report's paths are relative to; the default under `bazel test` is in `$TEST_TMPDIR`
  ([ADR 0031](0031-bazel.md)).
* The store is one file owned by `skif-cli`: a magic, a format number, the version and schema, and records sorted by
  key, so it is a deterministic function of its entries. Payloads are decoded only on a hit. A file that does not
  match is ignored as a whole and replaced.
* It is written after the run, whatever its exit code, only when the set of entries changed, and holds exactly the
  entries the run used. The write goes through a temporary file and a rename, with no lock; the last writer wins. A
  failed write is a warning and does not change the exit code.
* `--watch` keeps one cache across rounds (on disk with `--cache`), `skif lsp` one per root and `skif mcp` one per
  `(path, recursive)` of the `lint` tool, each pruned after a run. The servers never read or write the store.
  `explain-usage` runs without a cache.
* `SKIF_TIMINGS` and `--performance` print `js module cache <root>: H hits, M misses, W waits` and the time spent
  waiting as `js parse wait <root>`.

### Benchmarks

* `skif-bench run` checks the performance targets of [ADR 0001](0001-mission-scope-and-non-goals.md) and exits
  non-zero when one fails. It includes a generated workspace of 10,000 TypeScript files in 100 packages for the
  module graph target. `mise run bench` runs it locally, writes to `target/bench-results` and is never published.
* Published numbers come only from `.github/workflows/bench.yml`, run by `workflow_dispatch` on a pinned runner image
  label (`ubuntu-24.04` or `ubuntu-24.04-arm`, never `-latest`), with medians over measured runs after warmups.
  The workflow has read-only permissions and commits nothing.
* Everything compared is pinned: the corpus repositories by commit, the comparison tools by exact version in
  `corpus.toml` and a committed lockfile that must agree with it, the Rust toolchain by the measured ref's own
  `rust-version`, hyperfine and Node through `mise.lock`, and package managers through a pinned corepack. An unpinned
  input fails the run before anything is timed. No timed input comes from a cache.
* The summary records the machine, the runner, the toolchain, the measured and harness commits and how each corpus
  repository was installed, and the published tables print that record. A comparison tool's error makes a check
  `invalid`. A repository's `bench-knip-args` gives the timed knip run the arguments it needs to read the files skif
  reads.
* `mise run bench:import -- <run-id>` takes only a successful dispatched run of a commit on `main`, and
  `refresh-numbers.mjs` refuses a partial, dirty, outdated or invalid run. The maintainer commits the imported
  results, signed. The targets of ADR 0001 are shown in published runs but not enforced there; the type-aware budget
  below is.

### Large repositories

* `bench-max-runs` and `bench-max-warmup` in `corpus.toml` cap hyperfine's measured and warmup runs on a bench
  repository below a higher `--runs` and `--warmup`; a lower one is kept, so a quick dispatch stays quick. A cap of 0
  measured runs, or a cap outside the bench suite, is rejected.
* Every target records its warmup and measured runs in `summary.json`; `summary.md`, the provenance sentence and the
  claims of the published tables name the counts of a capped target, and an error count is out of the target's own
  runs.
* `bench-knip-config = "parity"` times knip with the configuration of the knip comparison
  ([ADR 0021](0021-knip-and-syncpack-comparison-harness-and-its-known-deviations.md)) in place of the repository's own,
  and skif with a `--config` that adds the same `entry` files, written by the same code the comparison writes them
  with. It is the only value, is rejected outside the bench suite, and a bench repository that sets `entry` must set
  it.
* A known knip failure on a corpus repository is declared (`bench-knip-failure`) and matched. A declared failure is
  shown as not measured, unless it sets `timed = true`: knip then exits with that error after printing its whole
  report, and a run that matches it is timed and compared with skif like a clean run and published with its reason. A
  declaration the run does not match makes the run invalid and unpublishable.
* The corpus checkouts keep their installs until every target is timed, so no install runs between two measurements.
  The workflow frees disk by removing toolchains no step uses before anything else, and reports the disk usage after
  the benchmark.

### Both modes and peak memory

* Each corpus and module graph target is timed with types (the default) and with `--no-type-aware` in the same
  hyperfine run. Each mode then runs once more under `/usr/bin/time` (GNU time's `-v` on Linux, `-l` on macOS) for its
  peak RSS; where neither is available the RSS is not recorded and its check is not measured.
* `summary.json` records the peak RSS on the rows of both modes and the budget checks, each `pass`, `fail`, `invalid`
  or `not-measured`. `summary.md` and the published tables show both modes, labelled "types on (default)" and "types
  off", with their peak RSS, and `refresh-numbers.mjs` refuses a summary that lacks a mode or the peak RSS of one.

### The type-aware budget

* Types off costs nothing against an untyped release: nothing of tsrs runs.
* Types on, measured on the benchmark runner, against the run of the same target with types off:
  * at most 3x the untyped run time on the largest corpus repository, the one whose untyped run takes longest;
  * a median of at most 2x across the corpus repositories;
  * at most 4 GiB peak RSS on every target;
  * at most 5 s on the generated 10,000-file repository;
  * at least 5x faster than knip wherever knip completes.
* The limits are `TYPE_AWARE_BUDGET` in `tools/bench/src/run.rs`. `--check-budget` makes `skif-bench run` exit
  non-zero when one fails, after writing the summary, and the `bench` workflow passes it, so the workflow fails a run
  that breaks the budget.
* Breaking the budget, or changing it, needs a new ADR.
* A program gets no more checkers than it has judged files, and the diagnostics a checker makes are freed with its
  program, so idle checkers and unread diagnostics add nothing to the peak.
* `SKIF_TIMINGS` prints the type-aware stages (programs, queries) and, per root, the checker memory in use and at most
  so far, so a run shows which roots the peak comes from.

### Consequences

* Good, because invalidation needs no dependency tracking: a changed file has a new key, and everything that is not
  a parse is recomputed.
* Good, because watch, the language server, the MCP server and multi-root runs parse only what changed or what is
  shared once.
* Good, because a published number names the commit, compiler, runner image and machine it came from, and can be
  repeated by dispatching the workflow.
* Good, because users see the cost of types in time and memory for both modes, and the speed claims cover the largest
  trees of the corpus with the same tool versions, machine and run as the others.
* Bad, because a warm run still reads and hashes every file, so the gain is bounded by the parse share of a run.
* Bad, because a narrower run prunes entries a full run needs, so alternating runs re-parse files.
* Bad, because `CACHE_SCHEMA` depends on a test and on review to be raised.
* Bad, because shared runners are noisy and an image label is eventually retired; ratios against the comparison tools
  on the same machine are the result, not absolute times, and a budget failure can be the runner's noise.
* Bad, because the workflow runs skif twice per target plus once per mode for the peak RSS, and the capped large
  repositories give a less steady median than the small targets.
* Bad, because a `timed` knip time is that of a run without the entries of the configuration it could not load, and
  the large repositories time knip with the comparison's configuration rather than their maintainers' own.
* Bad, because a fixed memory limit does not scale with the largest program: a repository with a much larger program
  can pass it, which is then a decision for a new ADR, not a bug.

### Confirmation

* `crates/skif-js-syntax/src/cache.rs`: `the_cache_schema_matches_the_parse_output`,
  `a_failed_parse_is_handed_to_its_waiters_and_stores_nothing`,
  `a_panicking_parse_leaves_one_waiter_to_parse_the_key_again` and
  `parse_cached_through_a_coalescer_stores_once_and_reports_parse_errors`, with the key and `MemoryModuleCache` tests.
* `crates/skif/tests/module_cache.rs`: `no_cache_a_cold_cache_and_a_warm_cache_report_alike`,
  `the_linters_of_several_roots_parse_a_file_they_share_once`,
  `linters_for_targets_report_like_a_linter_per_target` and
  `only_several_targets_or_a_host_cache_share_a_module_cache`.
* `crates/skif-cli/tests/cache.rs`: `no_cache_a_cold_cache_and_a_warm_cache_print_the_same_report`,
  `the_default_store_is_below_node_modules_of_the_root`, `a_store_that_cannot_be_written_is_a_warning_only`,
  `cache_location_requires_cache` and `a_recursive_run_parses_a_file_several_roots_import_once`.
* `tools/bench/src/provenance.rs` and `corpus.rs`: `the_machine_record_never_fails_and_names_the_architecture`,
  `rejects_a_comparison_tool_version_that_is_not_exact`, `the_shipped_tool_lock_installs_the_pinned_tools`,
  `the_bench_run_caps_and_the_comparison_configuration_are_read_and_checked` and
  `the_bench_repositories_whose_knip_run_fails_declare_the_error`.
* `tools/bench/src/run.rs`: `summary_markdown_is_formatted`,
  `a_matched_knip_failure_is_not_measured_and_a_stale_declaration_is_invalid`,
  `a_corpus_repository_caps_the_runs_of_its_commands_below_the_runs_asked_for`,
  `the_comparison_configuration_gives_knip_its_config_and_both_tools_the_entry_files`,
  `a_timed_knip_failure_is_checked_like_a_clean_run_with_its_reason`,
  `a_timed_knip_failure_keeps_its_times_and_ratio_in_the_summary`,
  `a_target_with_capped_runs_says_so_and_counts_its_errors_out_of_its_own_runs`,
  `a_run_within_the_type_aware_budget_passes_every_check`,
  `each_broken_limit_of_the_type_aware_budget_fails_its_own_check`,
  `an_unmeasured_peak_rss_is_not_measured_and_an_errored_mode_makes_its_checks_invalid`,
  `reads_the_peak_rss_that_gnu_and_bsd_time_print`, `the_peak_rss_run_is_the_timed_command_under_time` and
  `the_summary_shows_both_modes_and_the_budget_checks`.
* `tools/bench/refresh-numbers.test.mjs`: the cases of a knip error declared as after the report, of a target with
  capped runs, "a summary without both skif modes, or without the peak RSS of one, is refused", "an invalid type-aware
  budget check is refused and a failed one is published" and "the compact tables show both skif modes with their peak
  RSS"; and the `bench-import` cases of `scripts/test-check-scripts.sh`.
* `crates/skif-js-types/src/session.rs`: `a_program_gets_no_more_checkers_than_judged_files`;
  `crates/skif-js-types/tests/determinism.rs`: `every_program_reports_its_size_checkers_and_times`.
* `.github/workflows/bench.yml` passes `--check-budget`, so a dispatched run that breaks the budget fails.

## Pros and Cons of the Options

### Cache each file's parse result keyed by its content, share a run cache of non-local files with a parse coalescer, publish numbers of both modes from a manually dispatched workflow on a pinned GitHub-hosted runner, and fail that workflow on a broken type-aware budget

* Good, because the cache is correct by construction and the library does no I/O: an embedder supplies its own store.
* Good, because the results reach the repository through a signed local commit.
* Good, because the memory limit is one number the workflow checks without running another tool.
* Bad, because the cache, the coalescer, the provenance record and the two-mode measurement are code to maintain.

### Cache the whole report, keyed by the inputs of the run

* Good, because a warm run would cost almost nothing.
* Bad, because the inputs of a report are every file, configuration and environment value the run read, and a key
  that misses one returns a stale report.

### Cache parse results keyed by path, size and modification time

* Good, because a lookup needs no read of the file.
* Bad, because a same-size edit within the timestamp resolution, or a checkout that restores times, returns a stale
  result.

### Publish numbers measured on the maintainer's machine

* Good, because a quiet dedicated machine gives steadier numbers.
* Bad, because nobody else can repeat them, and the next publication may come from another machine without saying so.

### A scheduled benchmark workflow that commits its results

* Good, because numbers stay current without a manual step.
* Bad, because the workflow would need write access and its commits would not be signed by the maintainer.

### A memory budget relative to `tsc`'s peak on the largest program, measured in the workflow

* Good, because it scales with the program that drives the peak.
* Bad, because the workflow must install and run `tsc` on every repository, which costs more than the skif runs.
