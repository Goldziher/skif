---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0053"]
---

# Script Commands Run Where They Change Directory To, and the No-Entries Cause

## Context and Problem Statement

The knip comparison of 0.3.0 left two constructs where skif answered by accident rather than by a rule:

* A `package.json` script that changes directory before it runs a command (`"serve": "cd sub && pnpm exec rspack
  serve"`) was judged at the script's own package: `cd` was a shell builtin and the directory was not tracked. When
  `sub` is a workspace package that lists `@rspack/cli`, `rspack` was reported unlisted at the root and `@rspack/cli`
  could be reported unused in `sub`. knip has the same false finding.
* A package with no source file, entry or script of its own, whose manifest only lists what code elsewhere installs it
  for, is not judged for unused dependencies
  ([ADR 0082](0082-install-hooks-sveltekit-route-types-and-builtin-named-packages.md)). `incomplete-packages` gave it
  the cause `unmapped-published` ([ADR 0053](0053-incomplete-packages-in-the-json-report.md)), which says that
  `package.json` publishes build output with no source behind it, although such a manifest may publish nothing.

Where does a script command run, and what cause does a code-less manifest get?

## Decision Drivers

* A binary or package must be judged against the manifest whose dependencies the command uses
* A command whose directory is not known must keep its current judgement, so no new finding appears
* A cause in the report must state what skif found, so a reader can act on it

## Considered Options

* Track the directory of script commands and judge them in the workspace package there; add a cause `no-entries`
* Keep judging every script command at its own package, and keep `unmapped-published` for every unknown package

## Decision Outcome

Chosen option: "Track the directory of script commands and judge them in the workspace package there; add a cause
`no-entries`", because both settle a false or misleading answer without configuration.

**Script directories.** The script parser follows the directory of a command line: `cd <dir>` and `pushd <dir>` move
the commands after them to `dir`, relative to the directory they run in, until the next change, `popd` or the end of
the text. Another script, `sh -c` and a command substitution run in a shell of their own, so a `cd` in them ends with
them. The value of `npm --prefix <dir>`, `pnpm -C <dir>` (`--dir`, `--prefix`) and `yarn --cwd <dir>` moves the command
they run, where the parser already walks it (`exec`). A directory that is not a static relative path (`cd`, `cd -`,
`cd ~`, `cd /tmp`, `cd $DIR`) is not followed, and the commands after it are judged where the script starts. The
parser reports the directory with each binary and package (`BinaryUse::dir`, `DepRef::dir`) and resolves the files a
command names against it.

The JS analysis resolves the directory against the script's package. When the innermost workspace package holding it
is another package, the binaries the command runs and the packages it loads are credited to that package, as a binary
run with `pnpm --filter <name>` is: a listed provider there is used, and nothing is unlisted, since whether the name is
one of that package's scripts is not known from the caller. When the directory is in the package itself, in no
workspace package or outside the workspace, a command after `cd` stays a command of the script's package, as before,
and a command a package manager's directory flag runs stays unjudged, as before. The commands of tool configs,
workflow steps and source files keep their judgement; their directories are a later decision.

**The no-entries cause.** `IncompleteCause` gains `NoEntries`, written `no-entries`: the package has no source file
other than an install hook, no entry and no script, so its manifest only lists what code elsewhere installs it for.
A package whose published paths map to no source keeps `unmapped-published`; one that has both gets that cause only.
This amends the list of causes in the "Report data" paragraph of
[ADR 0053](0053-incomplete-packages-in-the-json-report.md).

### Consequences

* Good, because `cd sub && pnpm exec rspack` credits `@rspack/cli` in `sub` and reports nothing at the root, where
  knip reports `rspack` unlisted.
* Good, because a command after `cd` into a plain directory (`cd docs && vitepress build`) is judged exactly as before.
* Good, because a JSON consumer can tell a code-less manifest from unbuilt output.
* Bad, because a binary a workspace package does not provide is not reported for a command that runs there, as for
  `pnpm --filter`.
* Bad, because `npm run x` after `cd sub` still follows the script `x` of the script's own package.
* Neutral, because a JSON consumer that matched on `unmapped-published` for code-less manifests sees `no-entries`.

### Confirmation

* `crates/skif-js-plugins/src/scripts/tests.rs`: `commands_after_cd_run_in_its_directory`,
  `files_and_packages_after_cd_are_in_its_directory` and `directory_flags_of_package_managers_set_the_directory`.
* `crates/skif-js/src/scripts.rs`: `commands_run_in_another_directory_are_relocated`.
* `crates/skif-js/tests/monorepo_regressions.rs`: `commands_after_cd_into_a_workspace_package_are_judged_there`.
* `crates/skif-cli/tests/cli.rs`: `json_tells_a_package_without_entries_from_one_publishing_unmapped_output`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Track the directory of script commands and judge them in the workspace package there; add a cause `no-entries`

* Good, because the findings follow the manifest the command uses.
* Good, because a directory skif cannot place changes nothing.
* Bad, because the script parser models one more piece of shell state.

### Keep judging every script command at its own package, and keep `unmapped-published` for every unknown package

* Good, because nothing changes.
* Bad, because every `cd` into a workspace package needs an `ignore-binaries` entry and an `ignore-dependencies`
  entry, and the report keeps naming a cause that did not occur.

## More Information

* Related decisions: [ADR 0053](0053-incomplete-packages-in-the-json-report.md),
  [ADR 0069](0069-a-published-path-into-nothing-is-reported-not-an-incomplete-entry.md),
  [ADR 0076](0076-binaries-run-outside-scripts-and-unlisted-corrections.md),
  [ADR 0082](0082-install-hooks-sveltekit-route-types-and-builtin-named-packages.md)
* See the [ADR index](index.md).
