---
status: accepted
date: 2026-11-17
decision-makers: ["Na'aman Hirschfeld"]
---

# Benchmarks and the Comparison Harness

## Context and Problem Statement

skif replaces sherif, syncpack and knip and states speed targets
([ADR 0001](0001-mission-scope-and-non-goals.md)), so maintainers need two measurements on real repositories: how far
skif's findings agree with knip's and syncpack's, and how fast it is against them. The tools model some things
differently on purpose, so a raw one-sided count mixes intended differences with defects. A speed figure measured on an
unrecorded machine, with unpinned tools or on unequal work says nothing. Both measurements need the network and
installs, which a pull-request check should not depend on.

How is agreement measured and which run is counted, when is a one-sided finding an intended difference, where are
published speed numbers measured, and on what scope is each tool timed?

## Decision Drivers

* Results are reproducible: pinned repositories, pinned tool versions, the same scope for every compared tool
* Committed results carry counts only, never names from the compared repositories
* An intended difference is decided from evidence the harness can test on the checkout, counted and visible
* Anyone can repeat a published measurement from the repository alone, and no workflow commits
* The measurements are advisory: no release depends on them

## Considered Options

* A pinned corpus, parity and syncpack-diff tasks with count baselines and tested known deviations, a scheduled drift
  check, and published numbers only from a dispatched workflow on a pinned runner, each tool timed on its own scope
* A release gate on agreement, because the intended differences and any registry or install failure would block
  releases
* Known deviations decided by listing the findings to set apart, because a list matches only the repository it was
  taken from and says nothing about why the tools differ
* Numbers measured on the maintainer's machine, or a scheduled workflow that commits its results, because neither can
  be repeated from the repository alone and the second breaks signed commits

## Decision Outcome

Chosen option: "A pinned corpus, parity and syncpack-diff tasks with count baselines and tested known deviations, a
scheduled drift check, and published numbers only from a dispatched workflow on a pinned runner, each tool timed on its
own scope", because it gives a versioned measure of agreement whose one-sided counts mean something, and published
ratios that divide the times of equal work measured where anyone can repeat them.

### The corpus and the tool pins

* `tools/bench/corpus.toml` pins each repository to a full commit SHA and assigns it to the `bench` suite, the `parity`
  suite or both.
* The compared tools are pinned to exact versions in its `[tools]` table. `tools/bench/tools/package.json` and the
  committed `tools/bench/tools/package-lock.json` repeat them and pin everything they depend on, and the harness refuses a lock or an install
  that names other versions. `skif-bench tools update` moves every pin to the latest release and relocks;
  `skif-bench tools check` fails when a pin is behind, and both workflows run it.
* Every comparison runs the same locked tools, installed without lifecycle scripts, and no `npx` resolution or tool on
  `PATH` is used. Corpus installs also run without lifecycle scripts. A failed install or build does not skip a
  repository: both tools run on the checkout, recorded as not installed.
* Each checkout is locked while a command uses it, so two runs cannot interleave.

### Agreement with knip

* `skif-bench parity` (`mise run parity`) runs skif and knip with JSON output in each checkout and keys both reports
  per knip issue type; only issue types both tools have are compared, so class members are not. knip runs with an
  empty configuration in skif's scope, so a repository's own ignores hide nothing. The harness reads skif's incomplete packages and knip's load errors.
* The compared run is skif with `--type-aware`. A second pass with `--no-type-aware` classifies the findings types
  settle: a one-sided finding no other deviation explains is `type-resolved` when skif reports it only with types, or
  when knip reports it and only skif's untyped run does too. `--no-untyped-pass` skips the second pass, and then no
  baseline can be recorded.
* `tools/bench/baselines/type-aware.json` records per repository and rule the counts of both runs, and the parity
  baseline header records the mode skif ran in; a baseline of another mode is not comparable.

### Agreement with syncpack

`skif-bench syncpack-diff` (`mise run diff:syncpack`) runs skif's policy rules and syncpack on the policy fixtures and
the `parity` repositories, keyed by syncpack's status, the manifest and the dependency. A fixture's `skif.toml` is
translated to syncpack options for what both tools share; a fixture with an option syncpack lacks is skipped and
counted.

### Baselines and drift

* The baselines under `tools/bench/baselines/` hold counts only, with the tool version, the install state and the
  category set. `--check` fails when a count moves beyond a tolerance, a repository changes status, a fixture newly
  differs or the run was made another way.
* `.github/workflows/corpus.yml` runs both checks weekly and on dispatch, never on pull requests. No release depends on
  them; a baseline is refreshed when a difference is understood and accepted.

### Known deviations

* A one-sided finding is a known deviation only when a test the harness runs on the checkout explains it: it reads the
  checkout's files, manifests and TypeScript configurations, and scans imports with skif's own parser and plugin
  resolver, or with oxc where knip's visitor shape must be mirrored. A deviation is never decided by listing findings
  or by a field added to skif's report.
* Each deviation explains exactly one cause: a rule of the compared tool that skif does not share, or a decision of one
  of skif's records the compared tool does not follow. A difference with no testable rule, or one skif's analysis
  should close, stays counted.
* Each has a kebab-case id naming its cause, a side and a reason, its own count that `--check` ignores, and a fixed
  place in its side's order; the first that applies wins. A table skif uses is read from skif's crate, never copied.
* The deviations are listed only in code, `KNIP_DEVIATIONS` in `tools/bench/src/parity.rs` and `SYNCPACK_DEVIATIONS`
  in `tools/bench/src/evidence.rs`, and the published reports render them from there. Each is added with a positive and
  a near-miss test, must apply to any repository added later, and is removed once skif fixes its cause.

### Published speed numbers

* `skif-bench run` times skif and the compared tools with hyperfine and checks the targets of
  [ADR 0001](0001-mission-scope-and-non-goals.md), including a generated module-graph workspace. `mise run bench` runs
  it locally and is never published.
* Published numbers come only from `.github/workflows/bench.yml`, run by `workflow_dispatch` on a pinned runner image
  label, never `-latest`, with medians after warmups. It has read-only permissions and commits nothing.
* Everything timed is pinned: the corpus by commit, the tools by the lock above, the Rust toolchain by the measured
  ref's `rust-version`, hyperfine and Node through `mise.lock`, and package managers through a pinned corepack. An
  unpinned input fails the run before anything is timed. Every timed row passes `--no-cache`, so no timed input comes
  from a cache, except the labelled cache rows below.
* The summary records the machine, runner, toolchain, measured and harness commits and how each repository was
  installed, and the published tables print that record. `mise run bench:import` takes only a complete, successful
  dispatched run of a commit on `main`, refuses a partial, dirty, outdated or invalid run, and the maintainer commits
  the imported results, signed.
* Large repositories may cap their measured and warmup runs in `corpus.toml`, and the published tables name a capped
  target's counts. A known knip failure is declared and matched; a declaration the run does not match makes the run
  unpublishable.
* Each target is timed without types and with `--type-aware`, and each mode runs once more for its peak RSS. The
  summary and the published tables show both modes; the type-aware budget checked against them is
  [ADR 0010](0010-type-aware-analysis-on-tsrs.md)'s.

### Cache rows and the cache-diff gate

* Labelled cache rows time skif's own cache ([ADR 0007](0007-js-analysis-core-on-oxc.md)): cold with the write
  (overhead target at most 5% over `--no-cache`), warm, warm typed, and one edit untyped and typed. They are outside
  the targets of [ADR 0001](0001-mission-scope-and-non-goals.md) and the budget of
  [ADR 0010](0010-type-aware-analysis-on-tsrs.md), so a warm number never stands in for a compared one.
* `skif-bench cache-diff` lints the corpus uncached, cold, warm and after seeded edits, untyped and typed, and fails on
  any difference between the reports. It runs in CI next to the typed-versus-untyped diff, because a cache that changes
  a finding is a correctness defect, not a measurement.

### Timing scope

* Each compared row is timed on the scope the compared tool reads, so every published ratio divides the times of two
  runs over the same projects. `skif --no-js`, compared with sherif and syncpack, runs on the root workspace.
* The full `skif` row, compared with knip and checked by the targets and the budget, runs on the scope knip reads:
  without `--recursive` unless the repository configures knip itself to read nested projects that are not workspace
  members. `skif --type-aware` runs on the scope of the full row.
* Where a repository sets `recursive`, a full recursive run is timed as an informational row: it has no ratio and no
  check, and enters no published claim. `recursive` keeps its meaning for `parity`, which compares findings, not time.
* A repository's knip timing configuration and arguments make knip read the files skif reads, written by the same code
  the agreement comparison uses.

### Consequences

* Good, because agreement and speed are measured, versioned and visible in review when they change, and the committed
  files contain counts only.
* Good, because every intended difference has a tested rule and a reason in one place.
* Good, because the cache's speed is published apart from the compared rows, and a cache that changes a finding fails
  CI.
* Good, because a published number names the commit, compiler, runner image and machine it came from, can be repeated
  by dispatching the workflow, and compares runs of equal scope.
* Bad, because the classification is an approximation that can hide a real defect; deviations are therefore counted,
  not dropped.
* Bad, because the harness mirrors parts of skif's resolution and of knip's visitors and can drift from both.
* Bad, because the scheduled job is slow and depends on registries, and a change to the pins or category set needs the
  baselines rewritten.
* Bad, because shared runners are noisy and an image label is eventually retired, so ratios on the same machine are
  the result, not absolute times.

### Confirmation

* `tools/bench/src/corpus.rs` checks the pins, the tool lock, the run caps and the checkout locks;
  `tools/bench/src/tools.rs` covers `tools update` and `tools check`.
* `tools/bench/src/parity.rs`, `tools/bench/src/evidence.rs`, `tools/bench/src/type_aware.rs` and
  `tools/bench/src/main.rs` cover the compared run, the untyped pass and every deviation's positive and near-miss case;
  `tools/bench/src/syncpack.rs` covers the translation for syncpack.
* `tools/bench/src/diff.rs` covers the drift check and its tolerance.
* `skif-bench cache-diff`, run in CI, confirms that caching changes no report.
* `tools/bench/src/run.rs`, `tools/bench/src/provenance.rs` and `tools/bench/refresh-numbers.test.mjs` cover the timing
  scope, the informational recursive row, both modes, peak RSS, declared knip failures and the published record;
  `scripts/test-check-scripts.sh` covers the import.
