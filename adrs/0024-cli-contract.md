---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# CLI Contract

## Context and Problem Statement

The `skif` binary is how most users and every CI job meet skif. Scripts branch on its exit code, parse its JSON, and
rely on its flags; hooks run it with `--fix`; a fix that changes manifests leaves the lockfile stale until the package
manager installs. The CLI must behave the same from any directory inside a workspace, refuse to rewrite files where
nobody can review the change, and say exactly which processes it starts.

What does the CLI accept, what does a fix run do, and what is stable?

## Decision Drivers

* The result does not depend on the current directory
* A script can tell a repository with problems from a run that could not check it, and stdout stays machine-readable
* Flags are a typed layer of configuration, not a second configuration model
* Files are never rewritten in CI, and a fix leaves the repository installable
* skif's own process reads files only; every process it starts is named here
* The CLI surface is public API with its own compatibility rule

## Considered Options

* One documented contract: `skif [PATH]` lints, subcommands for everything else, flags as the `ConfigOverrides` layer,
  a CI-gated fix pipeline that re-lints and then installs, and the reporting contract's exit codes and JSON mode
* A `skif lint` subcommand with per-command configuration flags
* Leave fixing and installing to wrapper scripts

## Decision Outcome

Chosen option: "One documented contract: `skif [PATH]` lints, subcommands for everything else, flags as the
`ConfigOverrides` layer, a CI-gated fix pipeline that re-lints and then installs, and the reporting contract's exit
codes and JSON mode", because it lets a script rely on one shape for every run, keeps stdout machine-readable on every
path and makes the CI guard mean what it says. The exit codes and JSON mode are those of
[ADR 0022](0022-reporting-and-output-formats.md).

### Invocation

* `skif [PATH]` lints; there is no `lint` subcommand. The workspace root found from `PATH` (default `.`) drives
  configuration, reported paths, fix writes and the install step, whatever the current directory. A `PATH` with no
  `package.json` at or above it, or `-r`, lints every root below it
  ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)). Under `bazel run`, relative paths resolve as
  [ADR 0031](0031-bazel.md) says.
* Subcommands: `rules`, `schema`, `init`, `config`, `explain-usage`, `migrate`, `baseline`, `update`, `list`, `json`,
  `format`, `mcp` and `lsp`. The commands that print a report rather than findings (`rules`, `config`, `baseline`,
  `migrate`, `update`) take `--format human|json`. `skif format` takes a single `--format` of the lint formats,
  without `=FILE`.

### Lint flags

* Output and fixing: `--format FORMAT[=FILE]`, `--output`, `--fix`, `--fix-unsafe` (implies `--fix`), `--dry-run`,
  `--allow-remove-files`, `--fix-type`, `--select highest|lowest`, `--no-install`, `--no-prompt`.
* Limits: `--max-warnings`, `--fail-on-warnings`, `--max-issues`, `--max-show-issues`, `--no-exit-code`.
* Rules and ignores: `--rule ID=LEVEL`, `--ignore-rule`, `--ignore-package` (replaces the file's list),
  `--ignore-dependency` (adds an `[[ignore]]` entry to the file's), `--ignore-rule-for-package RULE:PACKAGE`,
  `--require-ignore-reasons` ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)).
* JavaScript analysis: `--no-js`, `--production`, `--isolate-workspaces`, `--strict`, `--include-entry-exports`,
  `--no-gitignore`, `--tsconfig`, `--use-tsconfig-files`, `--tags`. A mode flag turns a mode on, never off.
* Report selection: `--workspace`/`-W`, `--include`, `--exclude` and their shortcuts (`--dependencies`, `--exports`,
  `--files`, `--cycles`), and the instance filters. They narrow the report, not the analysis.
* Configuration and runs: `--config` and `--no-config` (they conflict), `-r`, `--watch` (conflicts with fixing),
  `--cache`, `--cache-location`, `--color auto|always|never`.
* Diagnostics of the run itself: `--trace-dependency` (instead of linting), `--debug`, `--performance`, `--duration`,
  `--memory`, all on stderr.
* Flags form the `ConfigOverrides` layer, the last of the layers
  ([ADR 0004](0004-configuration-model-and-layering.md)); an array flag replaces the configured array, except
  `--ignore-dependency`.
* `--color auto` colours only when stdout is a TTY, `NO_COLOR` is unset and there is no `--output`.
* `--no-prompt` never asks which alternative fix to apply; such a diagnostic stays unfixed unless `--select` decides.

### CI refusal

* `--fix` and `--fix-unsafe` without `--dry-run`, `skif format` without `--check`, and `skif update` without `--check`
  are refused with exit `2` and the error kind `ci-refusal` when `CI` is truthy: set, non-empty after trimming, and
  not `0`, `false`, `no` or `off` in any case. Linting is never affected.

### The fix run

* Lint, plan the fixes ([ADR 0009](0009-fixes-edits-and-package-json-formatting.md)), write them atomically, lint again,
  and report and exit from the second lint, so exit `1` means problems remain after fixing. A multi-root run re-lints
  the roots it changed and the roots whose configuration was written. A failed write stops the run before the re-lint
  and the install.
* `--dry-run`, with `--fix` or `--fix-unsafe`, plans the fixes as a fix run does and prints them as a unified diff; it
  writes no file and runs no install. The reports are still rendered to stdout and their files, and with JSON the
  `fix` member carries `dry-run` and `changes` ([ADR 0022](0022-reporting-and-output-formats.md)). Report and exit
  code are those of the unchanged tree, and a dry run is allowed in CI. `--dry-run` without a fixing flag is a usage
  error.
* Then, unless `--no-install`, the detected package manager installs in the workspace root, found through the
  platform's executable lookup so Windows `.cmd` shims resolve. The install is skipped when nothing changed or no
  package manager was detected; skif does not guess one. A failed install exits `2` and keeps the fixes.

### Processes and the network

* Linting and fixing open no network connection and resolve no version. The CLI starts two kinds of process: the
  package manager's install after a fix, and `git show` for `skif baseline check --base`
  ([ADR 0023](0023-baselines.md)). `skif update` is the one command that contacts the registry
  ([ADR 0026](0026-skif-update.md)). The library starts nothing.
* The CLI returns its exit code from `main`; nothing below it calls `std::process::exit`. The binary's panic hook is
  the one exception: it reports the panic (as an `internal` error document in JSON mode) and exits `2`.

### Stability

* The command-line surface, including its reference page `website/src/content/docs/reference/cli.mdx`, is public API:
  renaming or removing a flag or changing an exit code is a breaking change of `skif-cli`.

### Consequences

* Good, because scripts branch on three outcomes, and a hook failing on a broken `skif.toml` is distinguishable from
  a hook rejecting a commit.
* Good, because the CI guard does not fire on `CI=false`, and the fix path does not depend on the current directory.
* Good, because the processes skif starts are few and named, and each can be avoided (`--no-install`, `--base-file`).
* Bad, because every flag is a compatibility commitment.
* Bad, because the install step makes `--fix` slower and dependent on the user's toolchain, and a failed install
  returns `2` although the manifests were fixed.
* Bad, because deciding "truthy" differs from tools that treat any set `CI` as true.

### Confirmation

* `crates/skif-cli/src/args.rs`: `every_long_flag_is_in_the_cli_reference`.
* `crates/skif-cli/src/env.rs`: `ci_truthiness_table`.
* `crates/skif-cli/tests/cli.rs`: `clean_workspace_exits_zero`, `issues_exit_one_in_both_formats`,
  `a_directory_without_a_workspace_exits_two`, `fix_from_another_directory_writes_root_relative_files_only`,
  `fix_is_refused_only_when_ci_is_truthy`, `ci_does_not_affect_linting`, `fix_unsafe_implies_fix`,
  `install_runs_once_in_the_workspace_root_after_a_fix`,
  `install_is_not_run_without_fix_without_changes_or_with_no_install`,
  `install_is_skipped_when_no_package_manager_is_detected`, `failed_install_exits_two_but_keeps_the_fixes`,
  `color_always_emits_escapes_and_never_does_not`, `no_prompt_is_accepted_and_fixes_without_asking`,
  `dry_run_shows_the_fix_plan_and_writes_nothing`, `format_check_exits_one_and_format_writes_the_fix` and
  `a_missing_bazel_run_directory_exits_two`.
* `crates/skif-cli/tests/update.rs`: `writing_is_refused_when_ci_is_truthy_but_check_runs`.
* `crates/skif-cli/tests/watch.rs`: `watch_refuses_fix`.
* `crates/skif-cli/tests/filters.rs`: `strict_and_isolate_workspaces_in_the_configuration_equal_their_flags`.

## Pros and Cons of the Options

### One documented contract: `skif [PATH]` lints, subcommands for everything else, flags as the `ConfigOverrides` layer, a CI-gated fix pipeline that re-lints and then installs, and the reporting contract's exit codes and JSON mode

* Good, because the common case is the shortest command, and every outcome has one documented shape.
* Bad, because the CLI surface grows with every knip and syncpack flag skif accepts.

### A `skif lint` subcommand with per-command configuration flags

* Good, because every command would be explicit.
* Bad, because the common case gets longer, and flags would duplicate the configuration model.

### Leave fixing and installing to wrapper scripts

* Good, because the CLI would never start a process.
* Bad, because every user would write the same wrapper, and the lockfile would be left stale by default.
