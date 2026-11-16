---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# CLI Contract and Reporting

## Context and Problem Statement

The `skif` binary is how most users and every CI job meet skif. Scripts branch on its exit code, parse its JSON and rely
on its flags; hooks run it with `--fix`; a fix that changes manifests leaves the lockfile stale until the package
manager installs. Its findings reach terminals, code-scanning services, CI annotations, merge-request widgets and pull
request comments, each wanting another shape, and a pipeline often wants several from one run. Scripts need one
machine-readable contract on success and on failure, and some facts about a run are not findings on a line: packages
skif could not fully judge, the roots a multi-root run covered, and how far the type checker covered each package.

What does the CLI accept, what does a fix run do, how are reports rendered, and what is stable?

## Decision Drivers

* The result does not depend on the current directory
* Flags are a typed layer of configuration, not a second configuration model
* Files are never rewritten in CI, and a fix leaves the repository installable
* JSON is a versioned public contract, and stdout in JSON mode is always one parseable document
* Renderers are pure and reusable by hosts, and one analysis can produce every report a pipeline needs

## Considered Options

* `skif [PATH]` with subcommands, flags as the `ConfigOverrides` layer, a CI-gated fix pipeline that re-lints and
  installs, pure renderers with a repeatable `--format FORMAT[=FILE]`, a versioned JSON contract and three exit codes
* A `skif lint` subcommand with per-command configuration flags, because the common case gets longer and flags would
  duplicate the configuration model
* One format per run with `--output`, because a pipeline that needs two reports lints twice and the reports may
  describe different trees
* Leave fixing, installing and rendering to wrapper scripts and the library user, because every user would write the
  same wrapper and converter

## Decision Outcome

Chosen option: "`skif [PATH]` with subcommands, flags as the `ConfigOverrides` layer, a CI-gated fix pipeline that
re-lints and installs, pure renderers with a repeatable `--format FORMAT[=FILE]`, a versioned JSON contract and three
exit codes", because a script relies on one shape for every run, one analysis serves every consumer, and the CI guard
means what it says.

### Invocation

* `skif [PATH]` lints; there is no `lint` subcommand. The workspace root found from `PATH` (default `.`) drives
  configuration, reported paths, fix writes and the install step, whatever the current directory. A `PATH` without a
  workspace at or above it, or `-r`, lints every root below it
  ([ADR 0004](0004-workspaces-roots-and-package-managers.md)). Under `bazel run`, relative paths resolve as
  [ADR 0011](0011-nx-lerna-and-bazel.md) says.
* Everything else is a subcommand. The commands that print a report rather than findings take `--format human|json`.
  The full surface is the CLI reference page, `website/src/content/docs/reference/cli.mdx`.

### Flags

* Flags form the `ConfigOverrides` layer, the last layer of the configuration
  ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)). An array flag replaces the configured
  array, except `--ignore-dependency`, which adds an ignore entry.
* A mode flag turns a mode on, never off, except type-aware analysis: `--type-aware` and `--no-type-aware` both exist
  and conflict ([ADR 0010](0010-type-aware-analysis-on-tsrs.md)). The baseline commands and `skif explain-usage` take
  them too, so they judge as the lint run they stand for.
* Report selection flags narrow the report, not the analysis. Diagnostics of the run itself (`--debug`,
  `--performance` and their kin) go to stderr.
* `--color auto` colours only when stdout is a TTY, `NO_COLOR` is unset and there is no `--output`. `--no-prompt` never
  asks which alternative fix to apply; such a diagnostic stays unfixed unless `--select` decides.

### CI refusal

Writing fixes, `skif format` without `--check` and `skif update` without `--check` are refused with exit `2` and the
error kind `ci-refusal` when `CI` is truthy: set, non-empty after trimming, and not `0`, `false`, `no` or `off` in any
case. A dry run is allowed, and linting is never affected.

### The fix run

* Lint, plan the fixes ([ADR 0005](0005-rules-diagnostics-and-fixes.md)), write them atomically, lint again, and report
  and exit from the second lint, so exit `1` means problems remain after fixing. A multi-root run re-lints the roots it
  changed and the roots whose configuration it wrote. A failed write stops the run before the re-lint and the
  install.
* `--dry-run` with a fixing flag plans the fixes, prints them as a unified diff and writes nothing; report and exit
  code are those of the unchanged tree. `--dry-run` alone is a usage error.
* Then, unless `--no-install`, the detected package manager installs in the workspace root, found through the
  platform's executable lookup. The install is skipped when nothing changed or no manager was detected; skif does not
  guess one. A failed install exits `2` and keeps the fixes. This install and `git show` are the only processes the
  CLI starts ([ADR 0001](0001-mission-scope-and-non-goals.md)).
* The CLI returns its exit code from `main`; nothing below it calls `std::process::exit`. A panic is caught where the
  CLI is entered, reported (as an `internal` error document in JSON mode) and exits `2`.

### Renderers and several reports

* `skif-report` renders a `Report` to a `Write`. Renderers do no I/O of their own and never read the environment; the
  CLI resolves TTY, colour and CI and passes them as options. The formats include human, JSON, SARIF, GitHub
  annotations and Code Climate; the CLI reference lists them.
* `--format` is repeatable: `FORMAT` renders to stdout and `FORMAT=FILE` to that file, with at most one format on
  stdout. Without `--format`, the format is `human`, plus `github` annotations when `GITHUB_ACTIONS=true`. `--output
  FILE` is allowed only with exactly one format. Conflicting destinations are usage errors raised before anything is
  linted.
* Every format is rendered in memory first, so a failed rendering writes no file; files are written atomically, and
  stdout last. The stdout format, else the first, decides JSON mode and whether fixes may prompt.

### The JSON contract

* A report document carries a `version`, the tool version, a summary and the diagnostics in the deterministic order of
  [ADR 0005](0005-rules-diagnostics-and-fixes.md). A fix run adds what was applied, skipped and installed; a multi-root
  run adds its roots and the summaries add up to the run's.
* Adding a member is not breaking. Removing or renaming one, or changing its type or meaning, bumps `version`. Keys are
  kebab-case, paths are root-relative with `/`, and an extra member can never shadow a reserved key.
* In JSON mode stdout holds exactly one document and no other text, on every path. A failure is an error document with
  a `kind` (`config`, `workspace`, `io`, `usage`, `ci-refusal` or `internal`), a message and, when known, a location.
  Under `--watch` each run prints one document.

### Facts that are not findings

* `incomplete-packages` lists the packages whose unused files and dependencies skif could not fully judge, with their
  unjudged scope and the causes ([ADR 0008](0008-plugins-config-evaluation-and-commands.md)). The human renderer
  prints a note naming them and pointing at `skif explain-usage`.
* `type-aware` describes the type-aware analysis of the run: whether it was on, the checker, and each package's
  coverage with the reason when not typed. Type coverage is never a diagnostic and never changes the exit code; the
  human renderer prints one footer line when packages were partly or not typed, and SARIF carries the object in its
  run properties.
* Formats with no place for a fact that is not a finding (SARIF results, GitHub annotations) leave these facts out.

### Exit codes and stability

* `0`: no error remains beyond `--max-issues` and warnings stay within `--max-warnings` (or `fail-on-warnings` per
  root); `info` never counts. `1`: problems remain, after any fixes. `2`: skif could not run (invalid configuration, no
  workspace, bad arguments, a refused fix in CI, a failed render, write or install).
* `--no-exit-code` turns `1` into `0` and keeps `2`. The formats never change the exit code.
* The command-line surface, its reference page and the JSON contract are public API: renaming or removing a flag or
  changing an exit code is a breaking change of `skif-cli`, and the JSON shape has its own `version`.

### Consequences

* Good, because scripts branch on three outcomes, and one `JSON.parse` handles success and failure.
* Good, because the CI guard does not fire on `CI=false`, and the fix path does not depend on the current directory.
* Good, because one analysis produces every report, and hosts reuse the renderers into their own buffers.
* Good, because a consumer can tell why a package was not fully judged or typed without a diagnostic for it.
* Bad, because every flag and JSON member is a compatibility commitment.
* Bad, because the install step makes `--fix` slower and dependent on the user's toolchain, and a failed install
  returns `2` although the manifests were fixed.
* Bad, because report files are not replaced as one transaction: a write that fails midway leaves earlier files updated.
* Bad, because SARIF consumers differ in which optional fields they read; only what GitHub code scanning uses is
  guaranteed.

### Confirmation

* `crates/skif-cli/src/args.rs` checks that every long flag is in the CLI reference page and that both type-aware flags
  parse on lint, baseline and explain-usage; `crates/skif-cli/src/lint.rs` checks that they reach the overrides.
* `crates/skif-cli/src/env.rs` holds the CI truthiness table.
* `crates/skif-cli/tests/cli.rs` covers exit codes, the fix and install pipeline, CI refusal and `--dry-run`;
  `crates/skif-cli/tests/formats.rs` covers several reports in one run; `crates/skif-cli/tests/update.rs` and
  `crates/skif-cli/tests/watch.rs` cover the refusals of `skif update` and `--watch`.
* `crates/skif-report/tests/render.rs` and `crates/skif-report/tests/annotations.rs` cover the JSON contract, error
  documents, determinism of every format, incomplete packages and type coverage.
