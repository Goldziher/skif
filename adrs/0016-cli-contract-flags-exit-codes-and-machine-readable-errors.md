---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# CLI Contract: Flags, Exit Codes and Machine-Readable Errors

## Context and Problem Statement

`skif-cli` is the only crate that reads the environment, prints, prompts, writes files and spawns
processes ([ADR 0003](0003-library-first-embedding.md)), so everything a script, a CI job or a git hook can rely on about skif's
behaviour is decided there. Several decisions are spread over other records without a single
statement of the contract: output formats and exit codes ([ADR 0012](0012-reporting.md)), fix safety
([ADR 0008](0008-fix-and-edit-model.md)), configuration overrides ([ADR 0004](0004-configuration-model.md)) and the hooks ([ADR 0014](0014-git-hook-integration.md)).

The inherited binary shows what goes wrong without one:

* it refuses `--fix` whenever the `CI` variable is merely *set* (`std::env::var("CI").is_ok()`), so
  `CI=false` and `CI=` disable fixing as well;
* every failure, whether the repository has problems or the tool could not run, is
  `std::process::exit(1)`, so a script cannot tell a dirty repository from a broken invocation;
* in `--format json` mode, errors still print as coloured text, so a consumer that parses stdout
  gets invalid JSON exactly when something went wrong;
* after `--fix` it runs the package manager's install command, while [ADR 0001](0001-mission-and-scope.md) states that skif
  "never contacts a package registry". Both are true of the inherited behaviour and the documents
  never reconciled them.

## Decision Drivers

* A script must distinguish "the repository has problems" from "skif did not run"
* JSON consumers must be able to parse stdout in every case, including failures
* Fixing in CI is almost always a mistake, but the guard must not trigger on `CI=false`
* The lockfile must not be left stale by a fix, yet skif itself must not touch the network
* Every flag, exit code and message shape must be testable without a real registry or package manager

## Considered Options

* One documented contract: exit codes 0, 1 and 2, JSON-mode errors as JSON, a CI-gated `--fix`, and an opt-out install step after fixing
* Keep the inherited behaviour (exit 1 for everything, `CI` presence check, text errors)
* Two exit codes (0 and 1) with errors distinguished only by stderr text
* Never run the package manager; print a reminder to run it

## Decision Outcome

Chosen option: "One documented contract: exit codes 0, 1 and 2, JSON-mode errors as JSON, a CI-gated `--fix`, and an opt-out install step after fixing", because it lets a script tell a dirty repository from a broken run, keeps stdout machine-readable on every path, makes the CI guard mean what it says, and resolves the registry question by naming exactly which process talks to a registry and when.

### Invocation

`skif [PATH]` lints the workspace containing `PATH` (default `.`): the CLI finds the workspace root
as the nearest ancestor that declares a workspace ([ADR 0004](0004-configuration-model.md)) and uses it for configuration, every
reported path, every fix write and the install step, so the result does not depend on the current
directory. Subcommands: `skif init` writes a starter `skif.toml` with a `#:schema` directive,
`skif schema` prints the JSON Schema, and `skif rules` lists the registry ([ADR 0007](0007-rule-and-diagnostic-model.md)).

| Flag | Meaning |
| --- | --- |
| `--format <human\|json\|sarif\|github>` | output format ([ADR 0012](0012-reporting.md)); default `human`, or `human` plus `github` annotations when `GITHUB_ACTIONS=true` |
| `--output <FILE>` | write the rendering to `FILE` instead of stdout; requires `--format`, so `--format sarif --output skif.sarif` is the code-scanning form |
| `--fix` | apply `Safe` fixes ([ADR 0008](0008-fix-and-edit-model.md)) |
| `--fix-unsafe` | apply `Safe` and `Unsafe` fixes; implies `--fix` |
| `--select <highest\|lowest>` | which version a version alignment fix targets, overriding `[policy] prefer` for this run |
| `--no-install` | do not run the package manager after fixing |
| `--max-warnings <N>` | exit `1` when there are more than `N` warnings |
| `--fail-on-warnings` | `--max-warnings 0`; also set by `[lint] fail-on-warnings` |
| `--rule <ID>=<off\|info\|warn\|error>` | set a rule's level, repeatable |
| `--ignore-rule <ID>` | set a rule to `off`, repeatable |
| `--ignore-package <NAME>`, `--ignore-dependency <NAME>` | add to `ignore-packages` or add an `[[ignore]]` covering every rule, repeatable |
| `--ignore-rule-for-package <RULE:PACKAGE>` | an `[[ignore]]` for one rule on one package, repeatable |
| `--no-js`, `--production` | override `[js] enabled` and `[js] production` ([ADR 0010](0010-js-analysis-on-oxc.md)) |

Flags are the typed `ConfigOverrides` layer ([ADR 0004](0004-configuration-model.md)), applied over the file; arrays replace.
Colour is on only when stdout is a TTY, `NO_COLOR` is unset and the format is `human`; the CLI
decides and passes the result to the renderers.

### Exit codes

The meaning of `0`, `1` and `2` is defined in [ADR 0012](0012-reporting.md) and is part of this contract: `0` clean (warnings
within `--max-warnings`, `info` never counts), `1` problems remain, `2` skif could not run. Exit `2`
covers invalid or ambiguous configuration, an unreadable root or no workspace found, bad arguments
(clap's own usage errors already exit `2`), a refused `--fix`, and a failed install step. The CLI
returns an exit code from `main`; no function below it calls `std::process::exit`.

### `--fix` and CI

`--fix` and `--fix-unsafe` are refused, with exit `2`, only when `CI` is **truthy**: set, not empty
after trimming, and not one of `0`, `false`, `no` or `off` in any case. `CI=false`, `CI=0` and an
empty `CI` therefore allow fixing; `CI=true` and `CI=1` refuse it. The refusal is a normal
error (`kind` `ci-refusal` in JSON mode) that names the variable, so the cause is clear. Linting
without `--fix` is never affected.

### What a fix run does

1. Lint and build the `FixPlan` ([ADR 0008](0008-fix-and-edit-model.md)), selecting fixes by `--fix` or `--fix-unsafe` and `--select`.
2. Write the plan, atomically per file, with every path resolved against the workspace root.
3. Lint again on the changed tree; the report and the exit code come from this second run, so exit
   `1` means problems remain *after* fixing.
4. Run the install step, unless `--no-install` was given.

### Install after fix, and "never contacts a registry"

[ADR 0001](0001-mission-and-scope.md) says skif never contacts a package registry; this record states exactly what that
means. **skif's own process never opens a network connection and never resolves a version**: the
library cannot spawn anything, and a lint, with or without `--fix`, reads files only. The CLI has
one exception, a post-fix step: when a fix changed a manifest or catalog file and `--no-install` was
not given, it runs the detected package manager's `install` in the workspace root (never the current
directory), found through the platform's executable lookup so Windows `.cmd` shims resolve. The
network traffic that follows belongs to the user's package manager, not to skif.

The step exists because a fix changes what is declared, and leaving the lockfile stale would leave
the repository failing `--frozen-lockfile` installs. It is an opt-out because skif is also run
where installing is unwanted, and the git hooks pass `--no-install` ([ADR 0014](0014-git-hook-integration.md)). It is skipped, with
status `skipped`, when no file changed or when no package manager was detected ([ADR 0005](0005-workspace-discovery.md)); skif does
not guess one. If the install fails, the fixes stay written, the report still describes the
repository, and the exit code is `2`.

### JSON mode

With `--format json`, stdout holds exactly one JSON document and no other text, in every case,
including every failure: a report (with a `fix` object and an `install` status after a fix run), or
an `error` document with a `kind` (`config`, `workspace`, `io`, `usage` or `ci-refusal`), a message
and, when known, `path`, `line` and `column` ([ADR 0012](0012-reporting.md)). Usage errors that clap detects before skif
chooses a format are rendered as an `error` document too when `--format json` is on the command
line. `--output` never receives an error document; on failure the file is not written and the error
goes to stdout, so a script that redirects to a file still sees the failure.

### Consequences

* Good, because scripts can branch on three outcomes, and a hook failing closed on a broken `skif.toml` is distinguishable from a hook rejecting a commit.
* Good, because JSON consumers never meet text on stdout, so one `JSON.parse` handles success and failure.
* Good, because the CI guard no longer fires on `CI=false`, and the fix path no longer depends on the current directory.
* Good, because the registry claim is now precise and checkable: a lint opens no connection, and the only subprocess is an explicit, disableable package-manager step.
* Bad, because the CLI surface is now a public API: renaming a flag or changing an exit code is a breaking change of `skif-cli`, with its own changelog entry.
* Bad, because the install step makes `--fix` slower and dependent on the user's toolchain, and a failed install returns `2` even though the manifests were fixed.
* Bad, because deciding "truthy" ourselves differs from tools that treat any set `CI` as true; a CI system that sets `CI=0` to mean "not CI" is honoured, which is the intent but is a convention.

### Confirmation

* `cargo test -p skif-cli` includes `insta-cmd` snapshot tests that run the built binary for each row of the flag table and for `--help`.
* A table test asserts the exit code for each outcome: clean (`0`), errors remaining (`1`), warnings over `--max-warnings` (`1`), `info` only (`0`), invalid config, both config files, unreadable root, bad flag, refused `--fix` and failed install (all `2`).
* A table test sets `CI` to unset, empty, `0`, `false`, `FALSE`, `no`, `1`, `true` and `yes` and asserts which allow `--fix`.
* For every failure case above, a test runs with `--format json` and asserts stdout parses as exactly one JSON document with the expected `kind`; one run uses an invalid `skif.toml` and one uses an unknown flag.
* A test runs `--fix` from a subdirectory with a root-level issue in the fixture and asserts the root `package.json` was fixed and no file was created in the current directory.
* An install test puts a recording shim for the package manager first on `PATH` and asserts it was called once, with `install`, in the workspace root, only after a changed file, never with `--no-install`, never without `--fix`, and not at all when no package manager is detected; a Windows job asserts a `.cmd` shim resolves.
* The `e2e` job in `.github/workflows/ci.yml` runs the binary against real npm, pnpm, yarn and bun workspaces, including one `--fix` run per manager.
* A test runs `--format sarif --output <file>` and asserts the file holds valid SARIF, stdout is empty, and a failing run leaves the file untouched.

## Pros and Cons of the Options

### One documented contract: exit codes 0, 1 and 2, JSON-mode errors as JSON, a CI-gated `--fix`, and an opt-out install step after fixing

* Good, because every behaviour a script depends on is stated once and tested, and the registry claim becomes precise.
* Bad, because the flags and exit codes become a public API to maintain, and the install step ties `--fix` to the user's package manager.

### Keep the inherited behaviour (exit 1 for everything, `CI` presence check, text errors)

* Good, because it is what existing sherif users see and needs no work.
* Bad, because a script cannot tell a broken run from a dirty repository, `CI=false` disables fixing, and JSON output is invalid exactly when an error occurs.

### Two exit codes (0 and 1) with errors distinguished only by stderr text

* Good, because it matches the simplest linters and needs no documentation of a third code.
* Bad, because a hook that fails closed on a bad config looks the same as one that found a problem, and scripts would parse text to tell them apart.

### Never run the package manager; print a reminder to run it

* Good, because skif would spawn nothing and the registry claim would hold without qualification.
* Bad, because every fix would leave a stale lockfile until a human noticed, and CI with a frozen lockfile would fail on the commit that skif itself produced.

## More Information

* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0012](0012-reporting.md), [ADR 0014](0014-git-hook-integration.md)
* See the [ADR index](index.md).
