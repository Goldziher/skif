---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0059"]
---

# Binaries Run Outside Scripts: Commands of Tool Files and `child_process` Calls, and Corrections to Unlisted Dependencies

## Context and Problem Statement

`unlisted-binaries` judges the commands of `package.json` scripts only. The plugins read many more commands: the
`run` steps of GitHub workflows, Git hooks, `lint-staged` tasks, task runner configs, and, since
[ADR 0059](0059-twenty-one-more-plugins-and-commands-run-from-source.md), the calls of `execa`, `zx` and
`nano-spawn` in source files. Their binaries credit the dependency that provides them, but a binary that nothing
provides is dropped without a finding. knip reports such a binary at the file that runs it, and its core also reads
the calls of `node:child_process` (`execSync('…')`, `spawn('…', […])`) in every file it reaches, which skif does not
read at all. On the parity corpus (GitHub issue #97) this is nearly all of the about one hundred binaries only knip
reports: `sqlite3`, `openssl` and platform tools spawned by tests and scripts of mastra, n8n, vscode and opencode,
and tools that workflows and `lint-staged` configs run.

The same comparison of unlisted dependencies (GitHub issue #96) found four places where skif is wrong:
`@jest-environment jsdom` docblocks are not read as imports; `require.resolve('.bin/tool')` is reported as the
package `.bin`; a bare `@import '_code'` in a plain `.css` file is reported as the package `_code` although
`_code.css` sits beside it; and the `vscode` module of an extension's client package is unlisted although the
extension that contains it declares `engines.vscode`. A fifth is in binaries: a dependency named like a binary is
assumed to provide it even when its installed manifest has no `bin` (the `stripe` SDK beside `stripe listen`).

Which commands outside `package.json` scripts does `unlisted-binaries` judge, where is a finding reported, and how are
the unlisted-dependency corrections decided?

## Decision Drivers

* A binary a repository runs is the same dependency gap wherever the command is written
* A finding must point at the file that holds the command, where a user can fix or ignore it
* Reading `child_process` calls must not read files a second time
* Commands are read statically; a computed command is not guessed
* A tool the machine that runs a command brings (a CI runner image, a developer's shell) is no dependency gap
* The corrections follow how the runtime resolves the construct, not a corpus-specific exception

## Considered Options

* Report the binaries of every command a plugin reads at its file, and read `child_process` calls during the parse
* Report only the binaries of `child_process` calls
* Keep `unlisted-binaries` to `package.json` scripts

## Decision Outcome

Chosen option: "Report the binaries of every command a plugin reads at its file, and read `child_process` calls during the parse", because it closes the gap for every source of commands at once, matches knip's findings key for key, and costs no extra file reads.

**Commands of tool files.** A command a plugin reads from a file (a `PluginInput::Script` of a config, workflow or
hook, or the new `PluginInput::SourceScript` of a source file that `execa`, `zx` and `nano-spawn` emit) records each
binary it runs with that file (`PluginOutput::run_binaries`, `WorkspaceInputs::run_binaries`). A binary no listed
dependency, ancestor project or `ignore-binaries` entry provides is reported by `unlisted-binaries` at the file, with
the message "`<file>` runs `<binary>`, which no dependency provides". Binaries fetched on demand (`npx`, `pnpm dlx`),
run in another workspace (`pnpm --filter web exec x`), shell builtins and the global binaries of the script parser
are not recorded; a `PluginInput::Binary` that a plugin names without a command is still only a credit. A step with a
`working-directory` (`PluginInput::ScriptAt`) is unchanged: its binaries belong to the package in that directory and
are not recorded.

**CI steps and Git hooks run binaries of the machine.** The commands of CI workflows, Git hooks and task runners run
on a machine that brings its own tools: a workflow step calls `shellcheck` or `swiftc` from the runner image. As
knip's `optionalBinaries` does for the same plugins, a binary such a command runs by name is only a credit; one it
reaches through a package manager (`pnpm exec x`, `yarn x`, `npm exec x`) or spells as a `node_modules/.bin` path is
judged as above. `PluginSpec::optional_binaries` marks these plugins: `github-actions`, `azure-pipelines`, `husky`,
`lefthook`, `taskfile`, `ava`, `aws-cdk`, `buf`, `playwright` (its `webServer` command) and `vite-plus`; knip reads
the `hooks` of husky 4 without it, skif reads every husky hook alike. `lint-staged` tasks, the `prisma` seed and the
commands of source files run in the project's toolchain and are judged in full. A GitHub Actions step whose `shell`
(or the `defaults.run.shell` of its job or workflow) is not a POSIX shell (`pwsh`, `powershell`, `cmd`, `python`) is
not read at all: the script parser reads POSIX shell, and a PowerShell cmdlet is no binary.

**Shell scripts beyond one-liners.** Workflow steps hold whole shell scripts, so the script parser reads the
constructs they use: the body of a here-document (`node <<'EOF' … EOF`, `cat > ~/.netrc <<-EOF`) is input, not
commands; the patterns of a `case` item (`Linux|Darwin)`) are no commands and `esac` is a reserved word; and a
function the script defines (`name() { … }`, `function name { … }`) runs no binary when the script calls it, as knip
skips the functions a script defines.

**Commands of source files run in their package.** A command a source file runs is parsed with the scripts of the
package that owns the file, so `execSync('pnpm download')` follows the package's `download` script as the same
command in a script would, instead of naming a binary `download`.

**Global binaries.** The global binaries of the script parser follow the later list of knip's
`IGNORED_GLOBAL_BINARIES` (`osascript`, `taskkill`, `sysctl`, `hg`, `powershell.exe`, …) except the names an npm
package provides as a binary (`claude`, `railway`), and add compilers and the platform tools of Windows, macOS and
Linux distributions (`gcc`, `c++`, `xcode-select`, `signtool.exe`, `wmic`, `update-rc.d`, `ldd`). Like the rest of
the list they are reported only with `ignore-global-binaries = false`.

**`node:child_process` calls.** The parser records the literal commands of the calls of `exec`, `execSync`, `spawn`,
`spawnSync`, `execFile` and `execFileSync` imported from `child_process` or `node:child_process` (named, default,
namespace or top-level `require`) as `ModuleInfo::commands`, only in files that import the module. `exec` and
`execSync` take the command line; the other four take the file and its literal arguments, and a call with a literal
argument that is not a plain word is skipped, as for `execa`. `fork` runs a module, not a command. In every module
reachable from an entry that is neither ignored nor in a fixture directory, the commands are parsed with the script
parser: a binary fetched on demand credits its provider, a package the command names is used, and any other binary
is judged as above and reported at the source file. System binaries count only with `ignore-global-binaries = false`,
as for scripts.

**Test environment pragmas.** `@jest-environment <name>` and `@vitest-environment <name>` in the comments before the
first statement are side-effect imports of the environment's module, as knip reads them: Jest's `jsdom` is
`jest-environment-jsdom`, Vitest's `edge-runtime` is `@edge-runtime/vm`, `node` imports nothing, a path stays a path
and any other name is the package it names.

**Dot directories of `node_modules`.** A path below `node_modules/.bin`, `.vite`, `.cache` or any other entry whose
name starts with a dot names no package: npm package names never start with one.

**Bare imports of plain stylesheets.** A bare `@import` of a `.css` file is looked up beside the stylesheet first,
like those of Sass, Less and Stylus, and as a package only when no file there matches: the URL of a CSS `@import` is
relative to the stylesheet and `postcss-import` tries `./<name>` before `node_modules`.

**`engines` of an enclosing package.** The runtime an `engines` entry names provides its module to the package that
declares it and to the packages inside it (outside `--strict`), as the client and server packages of an editor
extension rely on the extension's `engines.vscode`.

**Binaries of dependencies with a known manifest.** A listed dependency whose unscoped name is the binary's provides
it only while its manifest is unknown (neither installed, linked nor locked). With the manifest known, its `bin` field
decides.

### Consequences

* Good, because binaries that workflows, hooks, tool configs and spawned commands run without a dependency are
  reported, at the file that runs them.
* Good, because `child_process` commands come from the parse skif already does, so no file is read twice, and only
  reachable files are judged, as for imports.
* Good, because the five corrections remove reports of names that are no packages (`.bin`, `_code`) and add the
  environment packages tests load.
* Neutral, because the module cache schema moves to 2: a store written by an earlier build is discarded.
* Good, because a tool a CI runner or developer machine brings, a PowerShell step, the body of a here-document, a
  `case` pattern and a shell function produce no finding.
* Bad, because repositories whose tool configs, source files, or CI steps through a package manager run tools that
  no manifest lists see new `unlisted-binaries` warnings; they add the provider or an `ignore-binaries` entry.
* Bad, because a tool a CI step runs by name that the project should list (a step that calls `tsc` after `pnpm
  install`) is not reported, as knip does not report it.
* Neutral, because a step on a Windows runner without a `shell` defaults to PowerShell on GitHub but is read as POSIX
  shell; its binaries by name are only credits.
* Bad, because a command built at run time (`` execSync(`git ${args}`) ``) is still not read.
* Bad, because a bare `@import` of a plain stylesheet that names a package also costs a lookup beside the file.

### Confirmation

* `crates/skif-js-syntax/tests/module_info.rs`: `child_process_calls_are_commands` and
  `test_environment_pragmas_are_imports_only_in_the_header`.
* `crates/skif-js-plugins/tests/plugins.rs`: `run_binaries_name_the_file_whose_command_runs_them`,
  `ci_and_hook_commands_judge_only_binaries_reached_through_a_package_manager_or_a_bin_path` and
  `github_actions_steps_in_a_shell_that_is_not_posix_are_skipped`.
* `crates/skif-js-plugins/src/scripts/tests.rs`: `here_document_bodies_are_input_not_commands`,
  `case_patterns_are_not_commands`, `functions_the_script_defines_are_not_binaries` and `ignored_binaries`.
* `crates/skif-js/tests/unlisted_parity.rs`: one test for each decision above.
* `crates/skif-js/src/specifier.rs`: the unit test `node_modules_packages` covers the dot directories.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Report the binaries of every command a plugin reads at its file, and read `child_process` calls during the parse

* Good, because one model covers scripts, tool files and source files.
* Bad, because a plugin that reads a command imprecisely now produces a finding where it produced a silent credit.

### Report only the binaries of `child_process` calls

* Good, because no plugin output changes.
* Bad, because the binaries of workflows, hooks and the `execa` family stay unjudged.

### Keep `unlisted-binaries` to `package.json` scripts

* Good, because nothing changes for users.
* Bad, because the gap to knip and the missed dependencies stay.

## More Information

* Amends: [ADR 0059](0059-twenty-one-more-plugins-and-commands-run-from-source.md) (commands read from source files
  are also judged by `unlisted-binaries`)
* Related decisions: [ADR 0010](0010-js-analysis-on-oxc.md),
  [ADR 0044](0044-monorepo-js-corrections-builtins-html-entries-referenced-paths-extglob-helpers-and-stylesheets.md),
  [ADR 0049](0049-js-corrections-from-adjudicating-knip-disagreements.md),
  [ADR 0068](0068-what-the-knip-comparison-sets-apart.md)
* See the [ADR index](index.md).
