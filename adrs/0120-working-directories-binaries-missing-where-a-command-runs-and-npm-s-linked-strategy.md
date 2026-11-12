---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0097"]
---

# Working Directories, Binaries Missing Where a Command Runs, and npm's Linked Strategy

## Context and Problem Statement

[ADR 0097](0097-commands-in-other-directories-and-binaries-of-workspace-members.md) credits a binary that a script,
a workflow step or a source file runs after `cd <member>` to that member, and judges member binaries by the package
manager. Four answers still did not follow where a command runs:

* A package that a workflow step, a tool config's command or a source file loads after `cd` (`run: cd packages/web &&
  node -r @x/loaded/register x.js`) stayed with the package where the command starts, so it was credited to the
  workflow's package and reported unused in `packages/web`, which lists it (GitHub issue #147).
* A step with a `working-directory` (`run: pnpm exec changeset`, `working-directory: packages/web`) recorded only the
  files it runs. The provider that `packages/web` lists was reported unused, while the same step written as
  `cd packages/web && pnpm exec changeset` credits it (#147).
* A binary credited to another workspace package that does not provide it was reported nowhere:
  `cd packages/web && mywdtool` is silent where `mywdtool` alone is reported. After `cd` into a project of its own
  below the workspace (a directory with a `package.json` that is no workspace member, often a nested root of
  `skif -r`), a binary was judged at the caller, although that project lists its provider (#163).
* npm with `install-strategy=linked` installs isolated, as pnpm does: a member's binaries are linked only into the
  packages that depend on it. skif took every npm workspace to hoist, so a script running a member's binary without
  listing the member had no finding and fails after a linked install (#162).

Where do the packages and binaries of a command in another directory belong, and what is reported when nothing there
provides a binary?

## Decision Drivers

* A command's packages and binaries are judged against the manifest of the directory the command runs in
* A step's `working-directory` and a leading `cd` are the same thing and get the same answer
* A binary missing where it runs is reported, at the command that runs it
* Only settings committed with the repository are read

## Considered Options

* Place packages and binaries by directory everywhere, report what nothing there provides, and read npm's
  `install-strategy`
* Keep the answers of ADR 0097 and leave the four cases to `ignore-dependencies` and `ignore-binaries`

## Decision Outcome

Chosen option: "Place packages and binaries by directory everywhere, report what nothing there provides, and read
npm's `install-strategy`", because each case is a false or a missed finding that the repository's files settle.

**Packages after `cd`.** A package that a workflow step, a tool config's command or a source file loads after `cd
<dir>` carries the directory (`DepRef::dir` of `skif-js-plugins`, `WorkspaceInputs::relocated_dependencies` of
`skif-js`) and is needed by the innermost workspace package that holds it, as a `package.json` script's is since ADR
0097. This replaces the sentence of ADR 0097 that the packages these commands name stay where the command starts;
their files are entries as before.

**Working directories.** A step that runs in another directory (`PluginInput::ScriptAt`: a GitHub Actions
`working-directory` of the step, or of the `defaults.run` of its job or workflow, an Azure Pipelines
`workingDirectory`, the `cwd` of a tool's command) is read as if it started with `cd <dir>`. Its binaries and packages
carry the directory and are placed as after `cd`.

**Binaries in another directory.** A binary that a command runs in another directory is a use there and no credit
where the command starts (`PluginOutput::referenced_binaries` leaves it out). A binary that a plugin's commands run by
name where its binaries are optional (a workflow step) or that is fetched on demand is still only a credit
(`BinaryRef::judged`, `RunBinary::judged`): one in another directory credits a provider there and is never unlisted.
Any other binary is judged where it runs:

* In another workspace package, a provider that package (or a package holding it) lists is used. When none does, the
  binary is unlisted at the command, and the finding names the package it runs in (``script `a` runs `mywdtool` in
  `packages/web`, which no dependency of `web` provides``, `UnlistedBinary::runs_in`), unless the package has a
  script of that name, which a package manager runs there (`cd web && pnpm bundle`). This replaces the answer of ADR
  0097 that nothing is unlisted at the file.
* In a project of its own between the workspace package and the directory (the innermost directory there with a real
  `package.json`), a provider the project's manifest lists is used: the report lists it among the
  `ancestor_dependency_uses`, so `skif -r` credits it to the root that owns that manifest. Otherwise a provider of the
  workspace package holding the project, as before, is used; else the binary is unlisted at the command, naming the
  project by its `name` or its directory. This applies also when that package is the one the command starts in.
* A directory outside the workspace root, or in the package itself and no project, leaves the binary where the
  command starts, as before.

**npm's linked strategy.** Under npm, a member's binary is provided to a package that does not list the member only
when npm hoists, as for pnpm in [ADR 0108](0108-member-binaries-under-pnpm-s-hoisted-node-linker.md): the last
top-level `install-strategy` of the root `.npmrc`, read as npm's `ini` parser reads it, decides, and `linked` keeps a
member's binaries to the packages that list the member and to the member's own scripts. `hoisted` (npm's default),
`nested` and `shallow` keep the earlier answer. A member's `.npmrc`, the user's `.npmrc`, `npm_config_install_strategy`
and `npm install --install-strategy` are not read.

### Consequences

* Good, because a package or binary that a workflow, a tool config or a source file uses in a workspace package is
  credited to that package, whether the step changes directory with `cd` or with `working-directory`.
* Good, because a binary that is missing where it runs is reported, and the finding says where that is.
* Good, because a project of its own that provides the binary, such as a nested root, counts it as used.
* Good, because an npm workspace that installs linked reports a member's binary that a package does not list.
* Bad, because a provider that the package where a workflow starts lists for a command in another directory is no
  longer used by that command; a repository that lists a tool only at the root and runs it in a member that lists it
  too sees the root's listing reported unused.
* Bad, because a bare binary after `cd` runs with the `PATH` of the script that started it; skif judges it in the
  directory, as ADR 0097 does, so a binary only the caller provides is reported there.
* Neutral, because whether npm's linked strategy still links member binaries into the root `node_modules/.bin` is not
  verified by an install: the end-to-end matrix of
  [ADR 0105](0105-yarn-classic-support-and-the-end-to-end-matrix-of-linkers-and-operating-systems.md) does not run it.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs`: `working_directory_steps_carry_the_directory_like_cd`.
* `crates/skif-js/tests/monorepo_regressions.rs`:
  `packages_loaded_and_binaries_run_in_another_directory_are_credited_to_the_package_there`,
  `binaries_run_in_another_package_that_does_not_provide_them_are_reported` and
  `npm_links_member_binaries_as_its_install_strategy_says`.
* `crates/skif-js/tests/fp_resolve.rs`:
  `binaries_run_after_cd_into_a_project_of_its_own_are_judged_against_its_manifest`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Place packages and binaries by directory everywhere, report what nothing there provides, and read npm's `install-strategy`

* Good, because every command is judged against the manifest of the directory it runs in, wherever it is written.
* Good, because the hoisting answer follows npm's committed setting, as pnpm's does.
* Bad, because a project of its own is found by reading the file system at each command's directory.

### Keep the answers of ADR 0097 and leave the four cases to `ignore-dependencies` and `ignore-binaries`

* Good, because nothing changes.
* Bad, because a false finding needs an entry per name, and a missing binary or member dependency stays unreported.

## More Information

* Amends: [ADR 0097](0097-commands-in-other-directories-and-binaries-of-workspace-members.md) (the "`cd` outside
  scripts" and "Member binaries" paragraphs)
* Issues: [#147](https://github.com/Goldziher/skif/issues/147), [#162](https://github.com/Goldziher/skif/issues/162),
  [#163](https://github.com/Goldziher/skif/issues/163)
* npm `install-strategy`: <https://docs.npmjs.com/cli/v11/using-npm/config#install-strategy>
* See the [ADR index](index.md).
