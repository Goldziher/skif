---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0094"]
---

# Commands in Other Directories and the Binaries of Workspace Members

## Context and Problem Statement

The knip comparison of 0.3.0 left four `unlisted-binaries` answers that did not follow where a command runs:

* A source file that runs `execa('bun', ['run', 'build'], { cwd })` runs the script `build` of the package in `cwd`.
  The call's options were dropped, so the command was read as one of the calling file and `build` was reported as an
  unlisted binary. A `package.json` script that runs `cd /tmp/app && bun run build` had the same answer, since a
  directory that is not a static relative path is not followed
  ([ADR 0094](0094-script-directories-and-the-no-entries-cause.md)).
* A workflow step that runs `cd sub && pnpm exec rspack` was judged at the workflow, although `sub` is a workspace
  package that lists `@rspack/cli`; ADR 0094 left the directories of workflow steps, tool configs and source files to
  a later decision.
* A binary that a workspace member declares in `bin` satisfied the script of any package, also one that does not list
  the member. pnpm, Yarn Berry and Rush link a member's binaries only into the packages that depend on it, so the
  script fails there.
* Rush runs its own commands (`rush`, `rushx`, `rush-pnpm`) from its install. They were satisfied only because a member
  of the Rush repository itself declares them, so the previous item would make them unlisted.

Where does a command run when skif cannot place its directory, where do the commands outside `package.json` run after
`cd`, and when does a workspace member's binary count as provided?

## Decision Drivers

* A binary must be judged against the packages whose binaries are on the `PATH` where it runs
* A command whose directory is unknown must not be charged to the caller for what belongs to the package there
* A change must not report a binary under a package manager that does provide it

## Considered Options

* Follow the directory and the package manager: unknown directories are another package's, `cd` outside scripts is
  tracked, and member binaries are provided where the package manager links them
* Keep the answers of ADR 0094 and list the four constructs in `ignore-binaries`

## Decision Outcome

Chosen option: "Follow the directory and the package manager", because each item is a false or a missed finding that
the repository facts already settle.

**Unknown directories.** After `cd` or `pushd` to a directory that is not a static relative path (`cd /tmp`,
`cd $DIR`, `cd -`, `cd ~`, a bare `cd`), and after any `cd` below such a one, the commands run in a directory the
script parser cannot place. A binary run there by name is still judged where the command starts, as in ADR 0094, since
the `PATH` it inherits is the caller's. A package manager run there (`npm run build`, `bun run build`, `pnpm build`,
`yarn compile`, `pnpm exec rspack`) works on the package of that directory, so it is handled as one with a filter or
`bun --cwd`: its scripts are not followed and the binaries it runs are not judged. `popd` and the end of a shell of its
own (a subshell `( … )`, `sh -c`, a substitution, another script) restore the directory, so
`(cd "$TMP" && tar xf a.tgz) && npm run build` follows `build`; a `{ … }` group runs in the same shell and keeps it.
This replaces, for package managers only, the sentence of ADR 0094 that the commands after such a directory are judged
where the script starts.

**The `cwd` option of a call.** A source file's call of `execa`, `execaNode`, `execaCommand`, nano-spawn or
`node:child_process` (`spawn`, `spawnSync`, `execFile`, `execFileSync`, `exec`, `execSync`) whose options object sets
`cwd` runs its command in that directory. A `cwd` that names the directory the call runs in anyway (`process.cwd()`,
`'.'`, `undefined`, `void 0`) changes nothing. A `cwd` the file's own location computes (`__dirname`,
`import.meta.dirname`, `path.dirname(fileURLToPath(import.meta.url))`, `path.join(__dirname, '../sub')`,
`new URL('../x', import.meta.url).pathname`) is that path below the file's directory: the arguments are evaluated a
second time from another directory, and a value that changes with it is one the file's location computes. The command
line is read as if it started with `cd` to it (`cd $__skif_file_dir__/../sub && rspack`, `ModuleInfo::commands`), and
the file's directory relative to its package takes the place of `$__skif_file_dir__` before the command is parsed, so
the command runs in a directory the parser can place, which may be another workspace package (see below). Any other
`cwd` (a literal path, which is relative to the process, a variable) runs the command in a directory the parser cannot
place: the command line starts with a `cd` to a substituted directory (`cd $__skif__ && bun run build`). An options
object skif cannot read, or one whose `cwd` only an unknown spread may set, leaves the command as before.

**`cd` outside scripts.** The binaries that a workflow step, a tool config's command or a source file run after `cd
<dir>` carry the directory (`BinaryRef::dir`, `RunBinary::dir`), relative to the package where the command starts: the
package the plugin runs for, which is the workspace root for a workflow. When the innermost workspace package holding
the directory is another package, the binary is credited to it as ADR 0094 credits a script's: a provider it lists is
used and nothing is unlisted at the file. In the package itself, in no workspace package or outside the workspace, the
binary is judged at the file, as before. A binary a package manager runs in the directory without naming a workspace
(`pnpm -C sub exec rspack`) is credited the same way, from a script, a workflow step or a source file alike, and never
judged at the file; one whose command names the workspaces it runs in (`cd sub && pnpm --filter web exec rspack`)
belongs to those and is not credited to the directory (`BinaryUse::relocation_dir`). The packages and files these
commands name stay where the command starts.
This settles the later decision ADR 0094 left open, for binaries. A workflow step starts in the checked-out
repository, `$GITHUB_WORKSPACE`, which the plugin takes to be the workspace root: `$GITHUB_WORKSPACE`,
`${GITHUB_WORKSPACE}` and `${{ github.workspace }}` in a step read as the root relative to the step's directory, so
`cd $GITHUB_WORKSPACE/sub` changes to `sub` and `node $GITHUB_WORKSPACE/scripts/x.js` runs that file. Any other
computed directory, such as `cd "$(dirname "$0")"`, stays one the parser cannot place.

**Member binaries.** A workspace member's binary is provided to a package that does not list the member only when the
package manager links every member's binaries into the root `node_modules/.bin`: npm, Yarn Classic and Bun, and an
unknown package manager, which keeps the old answer. Under pnpm, Yarn Berry and in a Rush repository (a `rush.json` at
the workspace root, whatever its package manager) it is provided only to the packages that list the member, to the
packages below one that does, and to the member's own scripts. In a Rush repository, `rush`, `rushx` and `rush-pnpm`
are provided commands; a package that lists the package declaring them still uses it.

### Consequences

* Good, because `execa('bun', ['run', 'build'], { cwd })` and `cd /tmp/app && bun run build` no longer report `build`.
* Good, because `cd sub && pnpm exec rspack` in a workflow credits `@rspack/cli` in `sub` and reports nothing at the
  workflow, as for a `package.json` script.
* Good, because a script that runs a member's binary without listing the member is reported under the package managers
  where it fails, and Rush's commands stay quiet in a Rush repository.
* Bad, because a binary that a package manager runs in an unknown directory is no longer reported where it is missing,
  as for `--filter`.
* Bad, because a pnpm workspace with `node-linker=hoisted`, or a Yarn Berry one whose scripts reach the root
  `node_modules/.bin`, now reports a member's binary that the install does provide; skif reads neither setting, and
  `ignore-binaries` silences such a binary.
* Bad, because a directory computed from the script's own location (`cd "$(dirname "$0")"`, `cd "${BASH_SOURCE%/*}"`)
  is not placed: the script parser does not know which file runs the command, so a package manager run there is
  handled as filtered and a binary run there by name is judged where the command starts.
* Neutral, because the commands that `ModuleInfo::commands` holds for a call with a `cwd` option start with
  `cd $__skif_file_dir__/<path> && ` or `cd $__skif__ && `, and the module cache schema moves to 3.

### Confirmation

* `crates/skif-js-plugins/src/scripts/tests.rs`:
  `package_managers_after_cd_to_an_unknown_directory_work_on_the_package_there` and `a_cd_in_a_subshell_ends_with_it`.
* `crates/skif-core/src/shell.rs`: `subshells_bound_the_segments_they_run`.
* `crates/skif-js-syntax/tests/module_info.rs`: `child_process_commands_with_a_cwd_change_to_an_unknown_directory`
  and `child_process_commands_with_a_cwd_of_the_process_or_the_file`.
* `crates/skif-js-plugins/tests/plugins.rs`: `commands_run_with_a_cwd_run_scripts_of_another_package` and
  `run_binaries_after_cd_carry_the_directory`, `commands_run_with_a_cwd_of_the_file_run_in_its_directory` and
  `cd_below_the_github_workspace_is_a_workspace_directory`.
* `crates/skif-js/tests/unlisted_parity.rs`: `commands_run_with_a_cwd_option_run_scripts_of_the_package_there` and
  `a_cwd_of_the_current_directory_follows_the_scripts_of_the_package`.
* `crates/skif-js/tests/monorepo_regressions.rs`: `a_member_binary_is_provided_where_the_package_manager_links_it`,
  `rush_commands_are_provided_in_a_rush_repository` and
  `workflow_and_source_commands_after_cd_into_a_workspace_package_are_judged_there` and
  `source_commands_with_a_cwd_of_the_file_run_in_that_directory` and
  `filtered_binaries_are_credited_to_their_directory_only_without_named_workspaces`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Follow the directory and the package manager

* Good, because the findings follow the `PATH` and the manifest the command uses.
* Good, because a hoisting package manager and an unknown one keep every earlier answer for member binaries.
* Bad, because the script parser models one more piece of shell state, and the hoisting answer rests on the package
  manager rather than on its linker settings.

### Keep the answers of ADR 0094 and list the four constructs in `ignore-binaries`

* Good, because nothing changes.
* Bad, because a false finding needs an entry per name, and a missing member dependency stays unreported.

## More Information

* Related decisions: [ADR 0076](0076-binaries-run-outside-scripts-and-unlisted-corrections.md),
  [ADR 0092](0092-machine-tool-binaries-unread-templates-and-run-time-entries.md),
  [ADR 0094](0094-script-directories-and-the-no-entries-cause.md)
* See the [ADR index](index.md).
