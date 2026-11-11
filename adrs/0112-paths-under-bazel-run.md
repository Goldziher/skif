---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0016"]
---

# Paths Under bazel run

## Context and Problem Statement

[ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) has `skif [PATH]` lint the workspace
containing `PATH` (default `.`), with `PATH` and every other relative path on the command line taken from the current
directory. `bazel run` starts a binary in its runfiles tree, not in the directory the user ran Bazel from, and names
that directory in `BUILD_WORKING_DIRECTORY` and the workspace root in `BUILD_WORKSPACE_DIRECTORY`.

With skif wrapped in a `native_binary` target, `bazel run //:skif -- --fix` linted the runfiles tree (only the files
the target lists as `data`), and the fix failed with `refusing to write through a symlink` because runfiles are
symlinks into the source tree (GitHub issue #171). Only an absolute path to the workspace worked.

How should skif resolve the paths on its command line when it runs under `bazel run`?

## Decision Drivers

* `bazel run //:skif -- --fix` lints and fixes the source tree the user is in, as buildifier and gazelle do
* Every relative path on the command line (`PATH`, `--config`, report files) resolves the same way, so a command
  means the same under `bazel run` as typed in a shell
* Outside `bazel run` nothing changes
* A host that embeds skif keeps control of its process: Node.js caches its current directory, so a change made
  behind its back leaves `process.cwd()` and worker threads out of step
* The environment is read only by the CLI crate ([ADR 0003](0003-library-first-embedding.md))

## Considered Options

* Change the current directory to `BUILD_WORKING_DIRECTORY`, else `BUILD_WORKSPACE_DIRECTORY`, before running
* Resolve only `PATH` against those directories
* Document passing an absolute path

## Decision Outcome

Chosen option: "Change the current directory to `BUILD_WORKING_DIRECTORY`, else `BUILD_WORKSPACE_DIRECTORY`, before
running", because it gives every relative path on the command line the meaning it has in the user's shell, with one
rule and no per-flag handling.

When `BUILD_WORKSPACE_DIRECTORY` is set and not empty, the `skif` binary changes the process's current directory,
before parsing the command line, to `BUILD_WORKING_DIRECTORY` when that is set and not empty (taken below the
workspace root if it is relative), else to `BUILD_WORKSPACE_DIRECTORY`. `BUILD_WORKING_DIRECTORY` alone does not
count as `bazel run` and changes nothing. A directory that cannot be entered is an `io` error with exit code `2`.
Absolute paths are unaffected. This applies to every command.

Only the binary does it, in `skif_cli::run_to_exit`; `skif_cli::run`, which the npm addon's `run` calls, never
changes the current directory, so an embedding host is not left out of step with its own idea of it. The npm
package's `skif` bin, the entry point `npx skif` and `bazel run` of an npm-installed skif start, makes the same
change in JavaScript with `process.chdir` before it calls `run`, and reports a directory it cannot enter on stderr
with exit code `2`. `lint` and `lintAsync` take an explicit path and do not change directory.

The statement of ADR 0016 that the result does not depend on the current directory still holds: under `bazel run`
the directory that `PATH` resolves against is the user's, not the runfiles tree.

### Consequences

* Good, because `bazel run //:skif` and `bazel run //:skif -- --fix` lint and fix the source tree, from the workspace
  root or from a subdirectory of it.
* Good, because `--config`, `--format FORMAT=FILE`, `--output` and the subcommands' paths resolve like `PATH`.
* Neutral, because a process that embeds the npm package and calls `run` itself under `bazel run` resolves paths
  against its own current directory, and enters the `bazel run` directory first if it wants the binary's behaviour.
* Bad, because the rule is written twice, in Rust for the binary and in JavaScript for the npm bin, and the two must
  be kept the same.

### Confirmation

* `crates/skif-cli/src/env.rs`: `the_bazel_run_directory_is_the_working_directory_else_the_workspace`.
* `crates/skif-cli/tests/cli.rs`: `bazel_run_fixes_the_workspace_of_the_working_directory`,
  `bazel_run_without_a_working_directory_starts_at_the_workspace`,
  `bazel_run_resolves_relative_paths_against_the_working_directory`,
  `bazel_run_resolves_a_relative_config_against_the_working_directory`, `bazel_run_leaves_absolute_paths_alone`,
  `a_working_directory_without_a_workspace_directory_is_ignored` and `a_missing_bazel_run_directory_exits_two`.
* `scripts/npm-smoke.sh`: the npm bin under `bazel run` lints `BUILD_WORKING_DIRECTORY`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Change the current directory to `BUILD_WORKING_DIRECTORY`, else `BUILD_WORKSPACE_DIRECTORY`, before running

* Good, because every relative path resolves as in the user's shell.
* Bad, because the current directory is process-wide state.

### Resolve only `PATH` against those directories

* Good, because nothing else in the process changes.
* Bad, because `--config` and report files would still resolve against the runfiles tree, so the same command line
  would mean two directories.

### Document passing an absolute path

* Good, because nothing changes.
* Bad, because `bazel run //:skif -- --fix` keeps failing, and the command differs per user and checkout.

## More Information

* Amends: [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) (the "Invocation" paragraph)
* Issue: [#171](https://github.com/Goldziher/skif/issues/171)
* Bazel `bazel run` environment: <https://bazel.build/docs/user-manual#running-executables>
* See the [ADR index](index.md).
