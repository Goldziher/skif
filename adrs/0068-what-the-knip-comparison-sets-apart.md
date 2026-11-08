---
status: accepted
date: 2026-11-08
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0030", "0053"]
---

# What the knip Comparison Sets Apart: Exports skif Does Not Judge, Configs knip Cannot Load, and Fixture Names

## Context and Problem Statement

[ADR 0053](0053-incomplete-packages-in-the-json-report.md) counts a finding only knip reports in `files`,
`dependencies` or `devDependencies` of a package skif lists under `incomplete-packages` as `not-judged-by-skif`, and
says that exports, types and members are still judged there. Since GitHub pull request #48 they are not:
`JsAnalysis::reports_exports_of` is false for every module of a package whose entries are incomplete, so
`unused-exports`, `unused-types`, the namespace rules and the member rules report nothing in it. Every knip-only
export, type and member in such a package counts as a difference although skif abstained on purpose.

Two more counts of the comparison mean something other than what they say (GitHub issue #27). knip prints
`ERROR: Error loading <file>` when a tool configuration of the repository does not load (create-t3-turbo's
`metro.config.js` requires an unlisted package, vue-core's `vite.config.ts` files read an undefined global), runs on
without the entry files that configuration names, prints its report and exits with 2. The harness compared that
report like any other, so findings that come from knip's missing entries counted as disagreement and the error was not
recorded. And [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md) names `fixture-file` "a fixture
directory skif skips", while the harness also counted `fixture`, `test_root`, `testdata`, `__testfixtures__` and
`*-fixtures` directories there, which [ADR 0050](0050-fixture-directories-in-the-js-analysis.md) leaves to skif's
ordinary judgement.

What does the knip comparison set apart from its agreement and difference counts?

## Decision Drivers

* A one-sided count must mean "unexplained", so an abstention or a tool failure must not land in it
* A tool failure must be visible in the report, not folded into the numbers
* A known deviation must describe what skif does, not what the harness assumes
* Baselines hold counts only, never names from the compared repositories

## Considered Options

* Extend `not-judged-by-skif` to every category skif abstains in, add a `knip-config-error` deviation, and match `fixture-file` to skif's fixture names
* Keep the categories and explain the gaps in the documentation
* Skip a repository whenever knip exits with 2

## Decision Outcome

Chosen option: "Extend `not-judged-by-skif` to every category skif abstains in, add a `knip-config-error` deviation, and match `fixture-file` to skif's fixture names", because each count then says what the tools did, and a repository whose knip run partly failed still contributes the comparison of everything the failure does not touch.

**Not judged by skif.** A finding only knip reports in `files`, `dependencies`, `devDependencies`, `exports`,
`types`, `nsExports`, `nsTypes`, `enumMembers` or `namespaceMembers` whose owning package (as in ADR 0053) is listed
under `incomplete-packages` is `not-judged-by-skif`. `unlisted`, `unresolved`, `binaries`, `duplicates` and
`optionalPeerDependencies` stay compared: skif judges them in every package. For `files` the category is an upper
bound: with the narrowed scope of [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md) skif judges the files
outside the scope, but its JSON does not say whether a package's scope is narrowed, so the harness counts every
knip-only file of a listed package here.

**Configs knip cannot load.** The harness reads knip's `ERROR: Error loading <file>` lines from stderr. A finding
only knip reports whose owning package holds such a file is the `knip-config-error` known deviation: counted, listed
in its column of the known-deviations section, recorded in the baseline under `known_deviations` and ignored by
`--check`. `not-judged-by-skif` and the file classifications take precedence. `parity.json` lists the files under
`knip_errors` with knip's exit code as `knip_exit`; the baseline records only their number, as `tool_errors`, and
`parity.md` shows it in the status cell ("compared; knip could not load 2 config files"). A knip run that prints no
report is still skipped with knip's error as the reason.

**Repositories that are never installed.** A repository with `install = false` in `corpus.toml` is compared
uninstalled under `--install` instead of being skipped before either tool runs, so a tool that cannot run there says
why itself: on rushstack, knip stops with `Unable to find package.json` and the entry is skipped with that reason.
`parity.json` records `installed` per repository.

**Fixture files.** `fixture-file` is a knip-only file below a `fixtures` or `__fixtures__` directory, the names of
`skif_config::FIXTURE_DIRECTORIES`. A knip-only file below another test-data directory is a difference unless it is
a test file or below a test directory (`test-file`).

### Consequences

* Good, because knip-only exports, types and members in packages skif deliberately does not judge no longer count as
  disagreement, and they stay counted and visible.
* Good, because a partial knip run is reported as one, with the files knip could not load.
* Good, because `fixture-file` now means what ADR 0030 says.
* Bad, because the committed parity baseline must be refreshed: `--check` against one recorded before this change
  reports the moved counts as drift.
* Bad, because `knip-config-error` attributes every knip-only finding of the package to the failed configuration,
  including findings that the configuration would not have changed.
* Bad, because the `files` part of `not-judged-by-skif` still includes files skif did judge in a package with a
  narrowed scope.

### Confirmation

* `skif-bench` unit tests: `not_judged_covers_exports_types_and_members_but_not_unlisted_or_duplicates`,
  `knip_only_findings_in_packages_skif_did_not_judge_split_off`, `reads_the_config_files_knip_could_not_load`,
  `classifies_unused_files_knip_alone_reports` and
  `the_summary_counts_not_judged_findings_apart_from_the_known_deviations`.
* The parity baseline in `tools/bench/baselines/parity.json` is recorded with these categories.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Extend `not-judged-by-skif` to every category skif abstains in, add a `knip-config-error` deviation, and match `fixture-file` to skif's fixture names

* Good, because the counts follow what each tool did on the run.
* Bad, because two more rules of the harness approximate ownership by `package.json` presence on disk.

### Keep the categories and explain the gaps in the documentation

* Good, because the baseline would not move.
* Bad, because the knip-only counts would keep mixing abstentions and failures with real differences, which a gate
  cannot tell apart.

### Skip a repository whenever knip exits with 2

* Good, because no partial knip result would be compared.
* Bad, because create-t3-turbo and vue-core would drop out of the comparison, although knip's report covers all but a
  few packages there.

## More Information

* Amends: [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md) (the knip deviations and the fixture
  names), [ADR 0053](0053-incomplete-packages-in-the-json-report.md) (the categories of `not-judged-by-skif`)
* Related decisions: [ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md),
  [ADR 0050](0050-fixture-directories-in-the-js-analysis.md),
  [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md)
* See the [ADR index](index.md).
