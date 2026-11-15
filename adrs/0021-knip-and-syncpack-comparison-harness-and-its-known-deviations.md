---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# knip and syncpack Comparison Harness and Its Known Deviations

## Context and Problem Statement

skif replaces knip and syncpack ([ADR 0001](0001-mission-scope-and-non-goals.md)), so how far its findings agree with
theirs on real repositories is a number maintainers need, and a change in it is a lead worth reading. The tools model
some things differently on purpose: skif follows the package manager's workspace semantics, judges files knip does
not read, abstains where it cannot know a package's entries, ranks ranges differently from syncpack, and resolves
member reads through the type checker ([ADR 0034](0034-type-aware-analysis-on-tsrs.md)). knip in turn reads some
files, imports and configurations in ways skif does not share. A raw one-sided count mixes those intended differences
with defects. The comparison also needs the network and installs, which a pull-request check should not depend on.

How is the agreement with knip and syncpack measured, which run of skif is counted, when is a one-sided finding set
apart as an intended difference, and what does the measurement gate?

## Decision Drivers

* Results are reproducible: pinned repositories, pinned tool versions, the same scope for both tools
* Committed results carry counts only, never names from the compared repositories
* The published agreement describes skif as it runs by default
* An intended difference is decided from evidence the harness can test on the checkout, counted and visible, never
  dropped
* What remains one-sided is unexplained, so a regression in it fails the drift check
* The measurement is advisory: no release depends on it

## Considered Options

* A pinned corpus, parity and syncpack-diff tasks against skif's default run, committed count baselines, known
  deviations decided by tested rules on the checkout, and a scheduled drift check
* A release gate on agreement with knip and syncpack
* Known deviations decided by listing the findings to set apart
* No comparison beyond the policy fixtures

## Decision Outcome

Chosen option: "A pinned corpus, parity and syncpack-diff tasks against skif's default run, committed count
baselines, known deviations decided by tested rules on the checkout, and a scheduled drift check", because it gives a
versioned measure of the default's agreement whose one-sided counts mean something, at the cost of one scheduled job.

### Corpus

* `tools/bench/corpus.toml` pins each repository to a full commit SHA and declares the exact versions of the compared
  tools in `[tools]`; a range, a dist-tag or a tool lock naming other versions is rejected.
* Each repository belongs to the `bench` suite, the `parity` suite or both, and may set `sparse`, `recursive`,
  `install`, `install-command`, `build-command`, their `*-timeout-secs`, `knip-root`, `knip-unread`, `entry`,
  `knip-invalid`, and the speed-comparison keys of
  [ADR 0029](0029-performance-caching-benchmarks-and-budgets.md).
* `skif-bench corpus fetch` (`mise run corpus:fetch`) fetches a suite at the pinned commits. Every command that uses a
  checkout takes an exclusive operating-system lock on `<name>.lock` beside it, so two runs cannot interleave.
* Installs run without lifecycle scripts. A Rush repository installs with `install-run-rush.js`, and for
  `knip-root = "rush"` the harness writes a root `package.json` (marked with `//`) from `rush.json` while knip runs. A
  failed install or build does not skip the repository: both tools run on the checkout, recorded as not installed.

### `mise run parity`

* `skif-bench parity` runs `skif --format json` and `knip --reporter json` in each checkout and keys both reports as
  `<file>|<name>` per knip issue type. Members are keyed without their `Enum.` or `ns.` qualifier.
* Class members are not compared, because the pinned knip has no class-member issue type.
* knip runs with an empty configuration in skif's scope, so a repository's own ignores hide nothing.
* `nsExports` and `nsTypes` hold skif's namespace rules ([ADR 0018](0018-export-namespace-and-member-rules.md)) and, on
  knip's side, the `exports` and `types` items that carry a `namespace` field, plus `nsExports`/`nsTypes` when present.
* The harness reads skif's `incomplete-packages` and their `scope` ([ADR 0022](0022-reporting-and-output-formats.md)),
  and knip's `Error loading <file>` lines.

### The counted run and `type-resolved`

* The knip comparison counts skif's default run, with types.
* A second run with `--no-type-aware` classifies the findings that types settle. A one-sided finding no other
  deviation explains is `type-resolved` when skif reports it only with types, or when knip reports it and only skif's
  untyped run does too. It is classified after every other deviation of its side.
* The baseline header records the mode skif ran in. A baseline of another mode is not comparable and is re-recorded;
  a baseline without a mode is compared.
* `tools/bench/baselines/type-aware.json` records per repository and rule the findings of both runs and those only one
  has, counts only, and `--check` checks it with the same tolerance as the other baselines.
* `--no-untyped-pass` skips the second run: `type-resolved` is then not classified, `type-aware.json` is not checked,
  and neither baseline can be recorded.

### `mise run diff:syncpack`

* `skif-bench syncpack-diff` runs skif's policy rules and `syncpack json` on the policy fixtures and the `parity`
  repositories, keyed by syncpack's status code, the manifest and the dependency.
* A fixture's `skif.toml` is translated to syncpack options for what both tools share; the translated
  `--dependency-types` gains `local` only in the default scope. A fixture with an option syncpack lacks is skipped and
  counted. The corpus runs use an empty syncpack configuration, and each Rush `projectFolder` becomes a `--source`.

### Baselines and drift

* `tools/bench/baselines/parity.json` and `syncpack.json` hold counts only, with the tool version, the install state
  and the category set. `--write-baseline` records a run; `--check` fails when a count moves by more than the larger
  of 10 % and 5, when a repository changes status, when a fixture newly differs, or when the run was made another way.
* A `knip-invalid` repository is compared and recorded, but its counts are left out of the totals.
* `.github/workflows/corpus.yml` runs both checks weekly and on dispatch, never on pull requests. No release depends
  on them; a baseline is refreshed when a difference is understood and accepted.

### Known deviations

The rule for when a one-sided finding is a known deviation, how a deviation is named and tested, and when it is
removed holds for both comparisons and for every repository. The deviations themselves are listed only in code:
`KNIP_DEVIATIONS` in `tools/bench/src/parity.rs` and `SYNCPACK_DEVIATIONS` in `tools/bench/src/evidence.rs` hold each
id with its reason, and the known-deviations sections of `parity.md` and `syncpack.md` are rendered from them.

When a finding is one:

* A one-sided finding is a known deviation only when a test the harness runs on the checkout explains it. The test
  reads the checkout's files, manifests and TypeScript configurations, and scans imports with skif's own parser and
  plugin resolver, or with oxc where knip's own visitor shape must be mirrored. A deviation is never decided by listing
  findings, and never by a field added to skif's report.
* Each deviation explains exactly one cause: a rule of the compared tool that skif does not share, or a decision of
  one of skif's own records that the compared tool does not follow.
* A difference with no testable rule stays counted. So does one that skif's own analysis should close.

How it is named:

* Each deviation has a kebab-case id that names its cause, not the finding: what knip does (`knip-…`) or the construct
  involved.
* Each deviation records its side (only the compared tool reports it, or only skif) and a reason.
* Each deviation has its own column and a count under `known_deviations` in the baseline, and `--check` ignores it.
* The order of the deviations is fixed per side, with the evidence closest to the finding (the file and its path)
  first. The first deviation that applies wins.
* A credit follows the `export … from` and `export *` re-exports of the files in between.
* A table skif's analysis uses, such as its global binaries or shell builtins, is read from skif's crate, never
  copied into the harness.

How it is tested and removed:

* Every deviation is added with unit tests in its harness module that pin its rule on a positive case and on a near
  miss.
* A deviation must apply to any repository added to the corpus later, not only to the one it was found in.
* A deviation is removed once skif fixes its cause.

### Consequences

* Good, because agreement with knip and syncpack is measured for the default run, versioned and visible in review
  when it changes.
* Good, because the committed files contain counts only.
* Good, because a one-sided count is a lead, and every intended difference has a tested rule and a reason in one
  place.
* Good, because a counted one-sided finding is one both modes agree on, so the counts move only with the analysis,
  not with the mode.
* Bad, because the classification is itself an approximation and can hide a real defect that looks like a deviation;
  deviations are therefore counted, not dropped.
* Bad, because the harness mirrors parts of skif's resolution and of knip's visitors and can drift from both, and a
  later knip may change the visitor shapes it mirrors.
* Bad, because the import scans parse more files of each checkout, which lengthens a parity run.
* Bad, because a finding that knip and the untyped run share and the typed run drops leaves the `both` count, and a
  check run with `--no-untyped-pass` counts `type-resolved` findings as one-sided and may report drift.
* Bad, because the scheduled job is slow and depends on registries, and a change to the category set or the pins needs
  the baseline rewritten.

### Confirmation

* `tools/bench/src/corpus.rs`: `shipped_manifest_is_valid_and_pinned`, `rejects_branch_pins`,
  `rejects_a_comparison_tool_version_that_is_not_exact`,
  `rejects_a_tool_lock_that_names_other_versions_than_the_manifest`,
  `no_repository_is_marked_as_an_invalid_knip_comparison`,
  `rushstack_installs_with_rush_and_runs_knip_in_a_synthesized_root` and
  `a_checkout_another_run_holds_fails_at_once_and_frees_when_released`.
* `tools/bench/src/diff.rs`: `check_accepts_small_drift_and_rejects_large`,
  `check_reports_a_repository_that_stopped_running`,
  `check_refuses_another_tool_version_or_category_set_instead_of_reporting_drift`, `check_flags_newly_differing_cases`,
  `check_ignores_known_deviations` and `check_refuses_another_analysis_mode_but_not_a_baseline_without_one`.
* `tools/bench/src/type_aware.rs`:
  `a_one_sided_finding_is_type_resolved_only_when_the_untyped_run_settles_it_the_other_way`,
  `the_rule_detail_gives_each_mode_its_count` and `the_baseline_names_the_untyped_run_as_its_tool_and_records_the_mode`;
  `tools/bench/src/main.rs`: `parity_runs_the_untyped_pass_unless_told_not_to`.
* `tools/bench/src/parity.rs`: `every_member_and_namespace_finding_agrees_with_the_json_knip_prints`,
  `the_recorded_category_set_holds_every_compared_category_and_deviation`,
  `earlier_deviations_take_precedence_over_the_layout_ones` and
  `type_resolved_takes_the_one_sided_findings_only_one_mode_has_after_the_other_deviations`, with a positive and a
  near-miss test per deviation, among them `binaries_rush_provides_shell_builtins_and_skif_s_global_binaries_split_off`
  (tables read from skif's crate), `members_only_skif_reports_whose_users_skif_does_not_read_split_off_on_skif_side`
  (credits through re-exports), `exports_of_the_files_skif_s_plugins_make_entries_of_a_nested_root_split_off` (skif's
  plugin resolver) and `import_expressions_knip_binds_and_those_it_reads_as_opaque_are_told_apart` (knip's visitor
  shapes with oxc).
* `tools/bench/src/syncpack.rs`: `translates_groups_and_policy`, `reports_what_has_no_syncpack_equivalent`,
  `syncpack_is_asked_for_the_local_instance_when_the_scope_selects_it` and `lists_the_packages_syncpack_read`.

## Pros and Cons of the Options

### A pinned corpus, parity and syncpack-diff tasks against skif's default run, committed count baselines, known deviations decided by tested rules on the checkout, and a scheduled drift check

* Good, because the numbers are reproducible, describe the default, and their changes are reviewed.
* Good, because a deviation found on one repository is a rule that applies to every repository.
* Bad, because the finding keys are approximations, and knip's results depend on its configurations loading.

### A release gate on agreement with knip and syncpack

* Good, because a regression could never ship unnoticed.
* Bad, because the intended differences would block releases, and a registry or install failure would too.

### Known deviations decided by listing the findings to set apart

* Good, because a deviation costs no code.
* Bad, because a list matches only the repository it was taken from, hides a regression that lands on a listed key,
  and says nothing about why the tools differ.

### No comparison beyond the policy fixtures

* Good, because there is nothing to maintain.
* Bad, because the replacement claim is unmeasured, and a regression on real repositories goes unseen.
