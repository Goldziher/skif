---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Scripts, Binaries and Working Directories

## Context and Problem Statement

A repository runs binaries from many places: `package.json` scripts, the `run` steps of CI workflows, Git hooks, task
runner files, tool configs, and the `execa`, `zx`, nano-spawn and `node:child_process` calls of source files. Each
command runs in a directory, often a different one from where it is written (`cd sub && pnpm exec rspack`, a
`working-directory`, a `cwd` option), and the binary it runs comes from the manifest whose dependencies are installed
there. `unused-dependencies` must credit the package that provides a binary, and `unlisted-binaries` must report a
binary that nothing provides, at the file that runs it and against the package where it runs.

skif reads these commands statically and runs nothing ([ADR 0001](0001-mission-scope-and-non-goals.md)). How does the
script parser read commands, which commands are judged, and in which package is a binary judged?

## Decision Drivers

* A binary a repository runs is the same dependency gap wherever the command is written
* A finding points at the file that holds the command, where a user can fix or ignore it
* A binary is judged against the manifest whose dependencies the command uses, which follows its directory
* A directory or command skif cannot place keeps a judgement that produces no false finding
* A tool the machine brings (a CI runner image, a developer's shell) is no dependency gap
* Reading commands costs no extra file reads, and a computed command is not guessed

## Considered Options

* Track the directory of every command, read commands from every source a plugin or the parser sees, and judge each
  binary in the innermost workspace package where it runs
* Judge only the binaries of `package.json` scripts, where the script is declared
* Track directories for `package.json` scripts only, and leave other commands as credits

## Decision Outcome

Chosen option: "Track the directory of every command, read commands from every source a plugin or the parser sees,
and judge each binary in the innermost workspace package where it runs", because it settles each case from facts the
repository already holds, and a directory skif cannot place changes nothing it judges.

### The script parser

* The parser reads POSIX shell. `cd <dir>` and `pushd <dir>` move the commands after them to `dir`, relative to the
  directory they run in, until the next change, `popd` or the end of the text.
* A subshell `( … )`, `sh -c`, a command substitution and another script run in a shell of their own, so a directory
  change inside them ends with them; a `{ … }` group runs in the same shell and keeps it.
* `npm --prefix <dir>`, `pnpm -C`/`--dir`/`--prefix <dir>` and `yarn --cwd <dir>` move the command they run.
* The body of a here-document is input, not commands; the patterns of a `case` item are no commands; a function the
  script defines runs no binary when the script calls it.
* In a GitHub Actions step, `$GITHUB_WORKSPACE`, `${GITHUB_WORKSPACE}` and `${{ github.workspace }}` are the workspace
  root. A step whose `shell` (or `defaults.run.shell`) is not a POSIX shell is not read.
* A directory that is not a static relative path (`cd /tmp`, `cd $DIR`, `cd -`, `cd ~`, `cd "$(dirname "$0")"`), and
  any directory below one, is unknown.

### Working directories outside the shell

* A GitHub Actions `working-directory` (of the step, or of `defaults.run`), an Azure Pipelines `workingDirectory` and
  the `cwd` of a tool's command act as a leading `cd`.
* The `cwd` option of an `execa`, nano-spawn or `node:child_process` call is read: a `cwd` of the current directory
  (`process.cwd()`, `'.'`) changes nothing; one the file's own location computes (`__dirname`, `import.meta.dirname`,
  `new URL('../x', import.meta.url)`) is resolved below the file's directory; any other `cwd` is an unknown directory.
  An options object skif cannot read leaves the command where it starts.

### Where a command's binaries and packages belong

* A command in a directory that another workspace package holds belongs to the innermost such package: the binaries it
  runs are credited there and the packages it loads are needed there, whether the command is a script, a workflow
  step, a tool config's command or a source file's call.
* A binary that package does not provide is reported by `unlisted-binaries` at the command, naming the package it runs
  in (`UnlistedBinary::runs_in`), unless the package has a script of that name, which a package manager runs there.
* A project of its own between that package and the directory (the innermost directory with a real `package.json`)
  is consulted first: a provider its manifest lists is used.
* A script that a package manager or task runner runs in another placeable directory (`cd web && npm run build`) is a
  script of the package there, never of the caller.
* A directory in the package itself, in no workspace package or outside the workspace root leaves the command where
  it starts.
* In an unknown directory, a binary run by name is judged where the command starts, since it inherits the caller's
  `PATH`; a package manager run there works on the package of that directory, so it is treated as filtered: its
  scripts are not followed and the binaries it runs are not judged.
* A binary run in a named workspace (`yarn workspace X`, `pnpm --filter X`, `npm -w X`) belongs to `X`: a provider `X`
  lists is used and nothing is unlisted, unless `X` has a script of that name. A command that names its workspaces is
  not also credited to its directory.
* Which packages a workspace member's binary is provided to depends on the package manager's linker
  ([ADR 0012](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md)).

### Which commands are judged

* `package.json` scripts, every command a plugin reads from a file (`PluginInput::Script`, and
  `PluginInput::SourceScript` for `execa`, `zx` and nano-spawn calls), and the literal commands of the `exec`,
  `execSync`, `spawn`, `spawnSync`, `execFile` and `execFileSync` calls of `child_process` in every reachable file that
  is neither ignored nor in a fixture directory. `fork` runs a module, not a command, and a command built at run time is
  not read. Calls are read during the parse the JS analysis already does.
* A command of a source file is parsed with the scripts of the package that owns the file, so `execSync('pnpm x')`
  follows the script `x`.
* A binary with no provider (no listed dependency, ancestor project or `ignore-binaries` entry) is reported at the file
  whose command runs it.
* Plugins whose commands run on a machine that brings its own tools (CI workflows, Git hooks, task runners) mark their
  binaries optional (`PluginSpec::optional_binaries`): a binary such a command runs by name is a credit only, while one
  it reaches through a package manager (`pnpm exec x`) or as a `node_modules/.bin` path is judged. Binaries fetched on
  demand (`npx`, `pnpm dlx`, `pnpm --filter … exec`) and a `PluginInput::Binary` a plugin names without a command are
  credits only.
* A listed dependency named like the binary provides it only while its manifest is unknown; once its installed, linked
  or locked manifest is known, its `bin` field decides.
* The global binaries of the script parser follow knip's `IGNORED_GLOBAL_BINARIES`, without the names an npm package
  provides, plus compilers and platform tools; they are reported only with `ignore-global-binaries = false`.

### Related imports

* `@jest-environment <name>` and `@vitest-environment <name>` pragmas before the first statement are side-effect
  imports of the environment's package (`jsdom` is `jest-environment-jsdom`; `node` imports nothing).
* A path below a `node_modules` entry whose name starts with a dot (`.bin`, `.cache`) names no package.
* A bare `@import` of a plain `.css` file is looked up beside the stylesheet before it is taken for a package.
* The runtime an `engines` entry names provides its module to the declaring package and to the packages inside it,
  outside workspace isolation ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)).

### Consequences

* Good, because `cd sub && pnpm exec rspack` credits `@rspack/cli` in `sub` from a script, a workflow or a source file
  alike, and reports nothing at the caller.
* Good, because a binary missing where it runs is reported at the command, and the finding names where that is.
* Good, because binaries that workflows, hooks, tool configs and spawned commands run without a dependency are
  reported, while a tool the CI image brings, a PowerShell step, a here-document body, a `case` pattern and a shell
  function produce no finding.
* Good, because `child_process` commands come from the existing parse, so no file is read twice.
* Bad, because a binary a package manager runs in an unknown directory is not reported where it is missing, as for
  `--filter`.
* Bad, because a directory computed from the script's own location is not placed: the parser does not know which file
  runs the command.
* Bad, because a provider listed only where a workflow starts does not cover a command that runs in a member that
  lists the same tool, so the root's listing can be reported unused.
* Bad, because a tool a CI step runs by name that the project should list is not reported, as knip does not report it.

### Confirmation

* `crates/skif-js-plugins/src/scripts/tests.rs`: `commands_after_cd_run_in_its_directory`,
  `files_and_packages_after_cd_are_in_its_directory`, `directory_flags_of_package_managers_set_the_directory`,
  `package_managers_after_cd_to_an_unknown_directory_work_on_the_package_there`, `a_cd_in_a_subshell_ends_with_it`,
  `a_script_run_in_another_directory_is_not_the_script_of_the_package`, `here_document_bodies_are_input_not_commands`,
  `case_patterns_are_not_commands`, `functions_the_script_defines_are_not_binaries` and
  `shell_builtins_are_neither_binaries_nor_global_binaries`.
* `crates/skif-core/src/shell.rs`: `subshells_bound_the_segments_they_run`.
* `crates/skif-js-syntax/tests/module_info.rs`: `child_process_calls_are_commands`,
  `child_process_commands_with_a_cwd_of_the_process_or_the_file`,
  `child_process_commands_with_a_cwd_change_to_an_unknown_directory` and
  `test_environment_pragmas_are_imports_only_in_the_header`.
* `crates/skif-js-plugins/tests/plugins.rs`: `run_binaries_name_the_file_whose_command_runs_them`,
  `ci_and_hook_commands_judge_only_binaries_reached_through_a_package_manager_or_a_bin_path`,
  `github_actions_steps_in_a_shell_that_is_not_posix_are_skipped`, `working_directory_steps_carry_the_directory_like_cd`
  and `cd_below_the_github_workspace_is_a_workspace_directory`.
* `crates/skif-js/tests/monorepo_regressions.rs`: `commands_after_cd_into_a_workspace_package_are_judged_there`,
  `workflow_and_source_commands_after_cd_into_a_workspace_package_are_judged_there`,
  `packages_loaded_and_binaries_run_in_another_directory_are_credited_to_the_package_there`,
  `binaries_run_in_another_package_that_does_not_provide_them_are_reported`,
  `scripts_run_in_another_directory_are_not_the_scripts_of_the_caller` and
  `filtered_binaries_are_credited_to_their_directory_only_without_named_workspaces`.
* `crates/skif-js/tests/fp_resolve.rs`:
  `binaries_run_after_cd_into_a_project_of_its_own_are_judged_against_its_manifest`.
* `crates/skif-js/src/scripts.rs`: `binaries_run_in_named_workspaces_are_forwarded`;
  `crates/skif-js/tests/graph_fixes.rs`: `binaries_run_in_a_named_workspace_use_the_provider_listed_above_it`.
* `crates/skif-js/tests/unlisted_parity.rs`: `commands_run_with_a_cwd_option_run_scripts_of_the_package_there` and
  `a_runtime_in_the_engines_of_an_enclosing_package_provides_its_module`.
* `crates/skif-js/tests/js_config.rs`: `ignore_global_binaries_false_reports_system_binaries`.

## Pros and Cons of the Options

### Track the directory of every command, read commands from every source a plugin or the parser sees, and judge each binary in the innermost workspace package where it runs

* Good, because findings follow the manifest a command uses, wherever the command is written.
* Good, because a directory skif cannot place changes nothing it judges.
* Bad, because the script parser models more shell state, and a plugin that reads a command imprecisely produces
  a finding instead of a silent credit.

### Judge only the binaries of `package.json` scripts, where the script is declared

* Good, because the model is small and nothing outside `package.json` produces a finding.
* Bad, because binaries that workflows, hooks and source files run without a provider are never reported, and every
  `cd` into a workspace package needs an `ignore-binaries` and an `ignore-dependencies` entry.

### Track directories for `package.json` scripts only, and leave other commands as credits

* Good, because the most common false finding (`cd sub && …` in a script) goes away.
* Bad, because the same command in a workflow step or a source file is judged differently, and binaries missing
  there stay unreported.
