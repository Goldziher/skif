---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# knip and syncpack Comparison Harness

## Context and Problem Statement

skif replaces knip and syncpack ([ADR 0001](0001-mission-scope-and-non-goals.md)), so how far its findings agree with
theirs on real repositories is a number maintainers need, and a change in it is a lead worth reading. The tools model
some things differently on purpose: skif follows the package manager's workspace semantics, judges files knip does
not read, abstains where it cannot know a package's entries, and ranks ranges differently from syncpack. A raw
one-sided count mixes those intended differences with defects. The comparison also needs the network and installs,
which a pull-request check should not depend on.

How is the agreement with knip and syncpack measured, what is set apart as an intended difference, and what does the
measurement gate?

## Decision Drivers

* Results are reproducible: pinned repositories, pinned tool versions, the same scope for both tools
* Committed results carry counts only, never names from the compared repositories
* An intended difference is classified from evidence the harness can check, counted and visible, never dropped
* What remains one-sided is unexplained, so a regression in it fails the drift check
* The measurement is advisory: no release depends on it

## Considered Options

* A pinned corpus, parity and syncpack-diff tasks with committed count baselines, known deviations classified from
  evidence, and a scheduled drift check
* A release gate on agreement with knip and syncpack
* No comparison beyond the policy fixtures

## Decision Outcome

Chosen option: "A pinned corpus, parity and syncpack-diff tasks with committed count baselines, known deviations
classified from evidence, and a scheduled drift check", because it gives a versioned measure of agreement whose
one-sided counts mean something, at the cost of one scheduled job.

### Corpus

* `tools/bench/corpus.toml` pins each repository to a full commit SHA and declares the exact versions of the compared
  tools in `[tools]`; a range, a dist-tag or a tool lock naming other versions is rejected.
* Each repository belongs to the `bench` suite, the `parity` suite or both, and may set `sparse`, `recursive`,
  `install`, `install-command`, `build-command`, their `*-timeout-secs`, `knip-root`, `knip-unread`, `entry`,
  `knip-invalid`, and for the speed comparison `bench-knip-args` and `bench-knip-failure`
  ([ADR 0029](0029-performance-caching-and-benchmarks.md)).
* `skif-bench corpus fetch` (`mise run corpus:fetch`) fetches a suite at the pinned commits. Every command that uses a
  checkout takes an exclusive operating-system lock on `<name>.lock` beside it, so two runs cannot interleave.
* Installs run without lifecycle scripts. A Rush repository installs with `install-run-rush.js`, and for
  `knip-root = "rush"` the harness writes a root `package.json` (marked with `//`) from `rush.json` while knip runs. A
  failed install or build does not skip the repository: both tools run on the checkout, recorded as not installed.

### `mise run parity`

* `skif-bench parity` runs `skif --format json` and `knip --reporter json` in each checkout and keys both reports as
  `<file>|<name>` per knip issue type; `classMembers` has no skif rule and is not compared. Members are keyed without
  their `Enum.` or `ns.` qualifier.
* knip runs with an empty configuration in skif's scope, so a repository's own ignores hide nothing.
* `nsExports` and `nsTypes` hold skif's namespace rules ([ADR 0018](0018-export-namespace-and-member-rules.md)) and, on
  knip's side, the `exports` and `types` items that carry a `namespace` field, plus `nsExports`/`nsTypes` when present.
* The harness reads skif's `incomplete-packages` and their `scope` ([ADR 0022](0022-reporting-and-output-formats.md)),
  and knip's `Error loading <file>` lines.

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

A one-sided finding is a known deviation when evidence the harness checks explains it. Each deviation has an id and a
reason in the harness, its own column, a count under `known_deviations` in the baseline, and is ignored by `--check`.
The first deviation that applies wins, in one order per side that the harness defines: the evidence closest to the
finding (the file and its path) first, `knip-config-error` last. The classes are:

* syncpack semantics skif differs from by decision: `satisfied-local-range`, `missing-local-version`,
  `floating-specifier`, `shorthand-ranking`, `npm-alias`, `unread-by-syncpack`;
* files outside skif's analysis or treated as entries: `non-code-file`, `declaration-file`, `test-file`,
  `fixture-file`, `skipped-directory`, `knip-unread`, `published-source` (a TypeScript source behind a published path,
  through build directories, extensionless paths, n8n node fields and the files those reach by relative import);
* what skif deliberately does not judge: `not-judged-by-skif` (in an incomplete package, within its scope);
* layouts knip reads otherwise: `knip-unjudged-manifest`, `non-member-project`, `rush-root-file`,
  `foreign-tsconfig-paths`, `bazel-build-file` ([ADR 0031](0031-bazel.md)), `entry-module-duplicate`,
  `signature-type`;
* findings downstream of a file disagreement: `in-knip-unused-file`, `in-skif-unused-file`,
  `used-in-knip-unused-file`, `used-in-skif-unused-file`, `imported-by-knip-unused-file`,
  `imported-by-skif-unused-file`;
* `knip-config-error`: a finding only one tool reports in a package with a configuration knip could not load.

A deviation is added only with a rule that decides it from reports, manifests or files and that would apply to a
repository added to the corpus later; one that skif fixes is removed from the harness.

### Consequences

* Good, because agreement with knip and syncpack is measured, versioned and visible in review when it changes.
* Good, because the committed files contain counts only.
* Good, because a one-sided count is a lead, and every intended difference has its reason in one place.
* Bad, because the classification is itself an approximation and can hide a real defect that looks like a deviation;
  deviations are therefore counted, not dropped.
* Bad, because the scheduled job is slow and depends on registries, and a change to the category set or the pins needs
  the baseline rewritten.
* Bad, because some deviations copy skif's lists (skipped directories, build directories) and can drift from it.

### Confirmation

* `tools/bench/src/corpus.rs`: `shipped_manifest_is_valid_and_pinned`, `rejects_branch_pins`,
  `rejects_a_comparison_tool_version_that_is_not_exact`,
  `rejects_a_tool_lock_that_names_other_versions_than_the_manifest`,
  `no_repository_is_marked_as_an_invalid_knip_comparison`,
  `rushstack_installs_with_rush_and_runs_knip_in_a_synthesized_root` and
  `a_checkout_another_run_holds_fails_at_once_and_frees_when_released`.
* `tools/bench/src/diff.rs`: `check_accepts_small_drift_and_rejects_large`,
  `check_reports_a_repository_that_stopped_running`,
  `check_refuses_another_tool_version_or_category_set_instead_of_reporting_drift`, `check_flags_newly_differing_cases`
  and `check_ignores_known_deviations`.
* `tools/bench/src/parity.rs`: `every_member_and_namespace_finding_agrees_with_the_json_knip_prints`,
  `unlisted_imports_only_foreign_tsconfig_paths_map_split_off`,
  `a_narrowed_scope_sets_apart_only_the_files_its_globs_match_and_every_dependency`,
  `knip_gets_a_root_manifest_in_a_rush_checkout_only_while_it_runs`,
  `files_a_published_source_reaches_through_files_only_knip_reports_unused_are_published_sources`,
  `findings_inside_a_file_the_other_tool_calls_unused_and_config_errors_split_off_on_both_sides`,
  `the_recorded_category_set_holds_every_compared_category_and_deviation` and
  `earlier_deviations_take_precedence_over_the_layout_ones`.
* `tools/bench/src/syncpack.rs`: `translates_groups_and_policy`, `reports_what_has_no_syncpack_equivalent`,
  `syncpack_is_asked_for_the_local_instance_when_the_scope_selects_it` and `lists_the_packages_syncpack_read`.

## Pros and Cons of the Options

### A pinned corpus, parity and syncpack-diff tasks with committed count baselines, known deviations classified from evidence, and a scheduled drift check

* Good, because the numbers are reproducible and their changes are reviewed.
* Bad, because the finding keys are approximations, and knip's results depend on its configurations loading.

### A release gate on agreement with knip and syncpack

* Good, because a regression could never ship unnoticed.
* Bad, because the intended differences would block releases, and a registry or install failure would too.

### No comparison beyond the policy fixtures

* Good, because there is nothing to maintain.
* Bad, because the replacement claim is unmeasured, and a regression on real repositories goes unseen.
