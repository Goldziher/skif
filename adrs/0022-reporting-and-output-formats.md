---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Reporting and Output Formats

## Context and Problem Statement

A run's findings reach terminals, scripts, code-scanning services, CI annotations, merge-request widgets and pull
request comments. Each wants another shape, and a pipeline often wants several from one run. Scripts need a stable
machine-readable contract, on success and on failure alike, and they need to tell a repository with problems from a
run that could not check it. Some facts about a run are not findings on a line: which packages skif could not fully
judge, and which roots a multi-root run covered.

How are reports rendered, what is the JSON contract, and what do the exit codes mean?

## Decision Drivers

* Renderers are reusable by hosts and testable without I/O ([ADR 0003](0003-library-api-and-embedding.md))
* GitHub users get annotations with no configuration
* JSON is a versioned public contract; stdout in JSON mode is always one parseable document
* Output is deterministic: the same tree gives byte-identical output
* One analysis can produce every report a pipeline needs
* A fact that is not a finding never counts against warnings or code-scanning dashboards

## Considered Options

* Pure renderers over a `Report`, a repeatable `--format FORMAT[=FILE]`, a versioned JSON contract with additive
  members, and three exit codes
* One format per run, chosen by `--format`, with `--output` for a file
* Leave rendering to the library user, with only JSON from the CLI

## Decision Outcome

Chosen option: "Pure renderers over a `Report`, a repeatable `--format FORMAT[=FILE]`, a versioned JSON contract with
additive members, and three exit codes", because hosts reuse the renderers, one run serves every consumer, and scripts
get one contract for every outcome.

### Renderers

* `skif-report` renders a `Report` to a `Write`. Renderers do no I/O of their own and never read the environment; the
  CLI resolves TTY, colour and CI and passes them as options.
* Formats: `human` (grouped by file, source excerpt, help and fix availability), `json`, `sarif` (2.1.0; columns in
  UTF-16 code units when the source text is known, else `unicodeCodePoints`), `github` (workflow-command
  annotations), `codeclimate` (GitLab Code Quality), `compact`, `markdown`, `disclosure` (markdown in collapsed
  sections) and `codeowners` (compact lines grouped by owner, which needs a CODEOWNERS file).
* `docs/rules.md` is generated from the registry's `RuleMeta` and checked for freshness.

### Several reports in one run

* `--format` is repeatable. `FORMAT` renders to stdout and `FORMAT=FILE` to that file. At most one format goes to
  stdout; with only file formats, stdout stays empty.
* Without `--format`, the format is `human`, plus `github` annotations when `GITHUB_ACTIONS=true`.
* `--output FILE` is allowed only with exactly one `--format FORMAT` and means `FORMAT=FILE`.
* Two stdout formats, the same file twice (after normalising the path), a directory target and an empty file are usage
  errors, raised before anything is linted.
* Every format is rendered in memory first; a failed rendering writes no file. Then files are written atomically, and
  stdout last, so a failed write reaches a JSON consumer of stdout as its only document.
* The stdout format, else the first `--format`, decides JSON mode, whether fixes may prompt and where the fix summary
  goes. A file rendering is never coloured.

### JSON contract

* A report document has `version` (1), `skif` (the tool version), `summary` (`errors`, `warnings`, `infos`, `fixable`,
  `packages`) and `diagnostics` (rule, severity, message, help, location with byte span and 1-based lines and
  scalar-value columns, related locations, package, dependency, specifier, detail and fixes).
* With `--fix`, a `fix` member holds `applied`, `skipped`, `changed` and `install`; under `--dry-run` it adds
  `dry-run` and `changes`.
* `incomplete-packages` lists the packages whose unused files and dependencies skif could not fully judge: `name`,
  `dir`, `scope` (`"package"` or `{"within": [globs]}`) and `reasons` (`cause`, `plugin`, `file`). Causes are
  `unparsable-config`, `unknown-config`, `unknown-spread`, `computed-entries`, `unmapped-published`, `no-entries` (no
  source file, entry or script of its own) and `unspecified`. The member is omitted when empty.
* A multi-root run adds `roots` (`path`, `packages`, `summary`; the summaries add up to the run's), and when they occur
  `errors` (roots that could not be linted) and `truncated` (directories the root search did not enter)
  ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)). Directories of a multi-root run carry the
  root's prefix.
* Adding a member is not breaking; removing or renaming one, or changing its type or meaning, bumps `version`. Keys
  are kebab-case, paths are root-relative with `/`, diagnostics follow the deterministic order of
  [ADR 0008](0008-rule-and-diagnostic-model-rule-ids-and-severities.md), and an extra member can never shadow a
  reserved key.
* In JSON mode stdout holds exactly one document and no other text, on every path. A failure is an error document,
  `error` with `kind` (`config`, `workspace`, `io`, `usage`, `ci-refusal`, or `internal` for a bug in skif, a panic
  included), `message` and, when known, `path`, `line` and `column`. Under `--watch` each run prints one document.

### Facts that are not findings

* The human renderer prints a note before the summary when packages are incomplete ("unused files and dependencies
  were not fully checked in …", naming the packages and pointing at `skif explain-usage`). SARIF and GitHub
  annotations do not carry it, since neither has a place for a fact that is not a finding.

### Exit codes

* `0`: no error remains beyond `--max-issues` (default 0) and warnings stay within `--max-warnings`, or within
  `fail-on-warnings` per root; `info` never counts.
* `1`: problems remain, after any fixes.
* `2`: skif could not run (invalid configuration, no workspace, bad arguments, a refused fix in CI, a failed render or
  write, a failed install).
* `--no-exit-code` turns `1` into `0` and keeps `2`. The formats never change the exit code.

### Consequences

* Good, because a host uses `Report` directly and can reuse any renderer into its own buffer.
* Good, because one analysis produces every report, and the reports always describe the same tree.
* Good, because one `JSON.parse` handles success and failure, and a script can tell problems from a broken run.
* Bad, because the JSON shape is a public API with its own version, maintained alongside the crate's semver.
* Bad, because the stdout and `--output` rules are checked after parsing, and a file name cannot hold a format's `=`.
* Bad, because files are not replaced as one transaction: a write that fails midway leaves earlier files updated.
* Bad, because SARIF consumers differ in which optional fields they read; only what GitHub code scanning uses is
  guaranteed.

### Confirmation

* `crates/skif-report/tests/render.rs`: `json_rich`, `json_incomplete_packages`,
  `json_incomplete_packages_say_whether_the_whole_package_is_unjudged`, `human_notes_incomplete_packages`,
  `json_error_document`, `detailed_error_document_carries_kind_and_location`,
  `extra_members_cannot_shadow_reserved_keys`, `json_is_deterministic` and `every_format_is_deterministic`.
* `crates/skif-report/tests/annotations.rs`: `sarif_structure`,
  `sarif_regions_use_scalar_columns_and_skip_missing_spans` and `github_rich`.
* `crates/skif-cli/tests/formats.rs`: `several_formats_go_to_their_files_and_one_to_stdout`,
  `two_formats_on_stdout_are_a_usage_error`, `output_with_several_formats_is_a_usage_error`,
  `the_same_file_spelled_twice_is_a_usage_error`, `a_directory_target_is_a_usage_error_before_any_write`,
  `only_the_stdout_rendering_is_coloured`, `a_failing_render_writes_no_file` and
  `the_stdout_format_decides_json_errors`.
* `crates/skif-cli/tests/cli.rs`: `issues_exit_one_in_both_formats`, `invalid_config_exits_two_in_both_formats`,
  `usage_errors_exit_two_and_are_json_in_json_mode`, `info_findings_alone_exit_zero`,
  `json_names_the_packages_whose_files_and_dependencies_were_not_judged` and
  `json_tells_a_package_without_entries_from_one_publishing_unmapped_output`.
* `crates/skif/tests/docs.rs`: `rules_reference_is_up_to_date`.

## Pros and Cons of the Options

### Pure renderers over a `Report`, a repeatable `--format FORMAT[=FILE]`, a versioned JSON contract with additive members, and three exit codes

* Good, because each value names its format and destination together, and the single-format forms are its one-value
  cases.
* Bad, because the CLI checks combinations clap cannot express.

### One format per run, chosen by `--format`, with `--output` for a file

* Good, because it is the simplest flag model.
* Bad, because a pipeline that needs SARIF and Code Climate lints twice, and the reports may describe different trees.

### Leave rendering to the library user, with only JSON from the CLI

* Good, because skif maintains one renderer.
* Bad, because every CI integration would write its own converter, and terminals get no readable output.
