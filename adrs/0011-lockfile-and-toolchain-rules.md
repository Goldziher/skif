---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Lockfile and Toolchain Rules: Lockfile Scopes, Stray Lockfiles, Patches and Pins Outside `package.json`

## Context and Problem Statement

Manifests say what a workspace asks for; the lockfile says what it installs, and Dockerfiles, CI workflows, version
files and shell scripts say which Node and which package manager build it. A banned package pulled in transitively, a
family split across releases in the lockfile, a patch that no longer applies after a bump, a leftover lockfile in a
member, a member that forms a workspace of its own, a Node version pinned in an image that differs from `.nvmrc`, and
a workflow still calling `npm ci` after a move to pnpm all pass a manifest-only check. These files are read by no
existing rule, and some of them, such as a member lockfile that a Bazel or Docker build installs from, are legitimate.

Which files outside `package.json` do the rules read, how, and what do they report?

## Decision Drivers

* A lockfile check that cannot run says so; it never passes silently
* A configuration that asks for no lockfile check reads no lockfile
* Rules read through the file system the host passes, so hosts linting in memory get the same findings
* A file a build installs from or a pin kept on purpose is not a finding, and a deliberate exception carries a reason
* Findings on these files report at `warn` by default, so adopting the checks does not fail builds that do not pass
  `--fail-on-warnings`
* Lockfile checks reuse the rule IDs and group shapes of the version policies

## Considered Options

* Rules over a lockfile model read on demand and over one shared scan of toolchain files, with allow-lists and
  comment suppressions that carry reasons
* Extend the manifest rules to these file kinds
* Leave lockfiles and toolchain files to dedicated tools

## Decision Outcome

Chosen option: "Rules over a lockfile model read on demand and over one shared scan of toolchain files, with
allow-lists and comment suppressions that carry reasons", because each finding is reported where it is fixed, a run
that asks for none of it pays nothing for the lockfile, both toolchain rules read each file once, and exceptions are
recorded with their reasons.

### The lockfile model

* `skif-workspace` models `pnpm-lock.yaml` in lockfile format 9: importers with their direct dependencies, the keys of
  `packages:`, snapshots with their dependencies and patch hash, `patchedDependencies` and `overrides`. A
  `pnpm-lock.yaml` of pnpm 11 and later that holds two YAML documents, the package manager's environment and the
  project's lockfile, is read as the project's document; the environment document is ignored. Every document must be
  format 9. Any other format or shape is `InvalidLockfile`.
* In a Bun workspace the lockfile is `bun.lock`, modelled by `BunLockfile`; a dependency resolves as Bun looks it up
  ([ADR 0012](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md)). The binary `bun.lockb` is not read.
  `yarn.lock` and `package-lock.json` are not modelled.
* Discovery does not read the lockfile. The rules read it on demand through `LintContext::fs`, the file system a host
  passes with `RunOptions::with_fs`, and share one parse per run through `LintContext::shared`.
* When a lockfile check cannot run (no lockfile, another manager's lockfile, a lockfile that does not parse, a
  `bun.lockb`-only Bun workspace), its rule reports why at its usual severity.

### Lockfile scopes

* A `[[version-groups]]` entry takes `scope = "manifests"` (default) or `"lockfile"`. A lockfile-scoped group is valid
  only with `policy = "banned"` and rejects `dependency-types` and `specifier-types`. It claims no manifest instance.
  `banned-dependency` reports each resolved `name@version` it matches once, at its lockfile entry, with the importer
  and the shortest chain that pulls it in; a direct dependency is a resolution like any other.
* A `[[dependency-families]]` entry takes `scope = "manifests"` (default), `"importer"` or `"lockfile"`. In a lockfile
  scope the members each importer reaches, or every snapshot, must be on one version, or on one version per major of an
  optional `anchor`. A family in a lockfile scope is not compared on the manifests, and moving a built-in family to one
  replaces it by name.

### `stray-lockfile` and `nested-workspace-root`

* Discovery records the lockfiles and `pnpm-workspace.yaml` files of the root and of every directory the workspace
  patterns select, leaving out files git ignores. Directories below a member that no pattern selects are not searched.
* `stray-lockfile` (workspace, warning) reports every lockfile in a selected directory and every root lockfile the
  detected manager does not read; without a detected manager, root lockfiles are not judged. A root `package-lock.json`
  next to `npm-shrinkwrap.json` and a root `bun.lockb` next to `bun.lock` are stray, since the manager ignores them.
  Its fix deletes the file: `Unsafe`, and it needs `--allow-remove-files`
  ([ADR 0009](0009-fixes-edits-and-package-json-formatting.md)).
* A member lockfile that a build installs from is not stray: one a Bazel label of the main repository or a `BUILD`
  file at or above it names ([ADR 0031](0031-bazel.md)), one a Dockerfile `COPY` or `ADD` names or matches, or one a
  Dockerfile of its directory copies with its whole context before an install by the lockfile's manager. A
  `docker build`, `docker buildx build`, `docker image build` or `podman build` in a script or workflow, and a root
  Compose file's service `build`, give a Dockerfile elsewhere the member's directory as its context. Finding them reads
  only the files Bazel reads for the lockfile, the Dockerfiles of the toolchain scan and those such builds name, and
  nothing at all without a member lockfile. Nothing is run.
* `nested-workspace-root` (workspace, warning, no fix) reports a `pnpm-workspace.yaml` in a selected directory, under
  any manager, and under Bun a member whose `package.json` declares `workspaces`; a Yarn or npm member with
  `workspaces` is not reported. A member inside a Bazel convenience link is reported once, with `detail`
  `BazelConvenienceLink`, and excluded from analysis. The file may hold settings that belong in the root, so there is
  no deleting fix.
* Both rules take `allow`: strings or `{ name, reason }` tables, relative to the workspace root. An entry is matched
  literally first, then as a glob where `*` crosses `/`; `!` and `$LOCAL` are rejected. An entry that matches nothing
  the rule would report is stale and reported at the configuration file, unless the file is inherited from above the
  root. `--require-ignore-reasons` covers these entries too
  ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)).

### `stale-patched-dependencies`

* `stale-patched-dependencies` (workspace, warning, no fix) reads pnpm's `patchedDependencies` from the root
  `pnpm-workspace.yaml` and `package.json#pnpm`, and under Bun the top-level `patchedDependencies` of the root manifest.
* It reports a patch whose release the lockfile does not resolve or does not patch, a missing patch file, a file outside
  `allowed-patch-paths` (globs, empty allows any), and a `*.patch` file directly in a patch directory that no entry
  references (`patch-dirs`, else the directories of the referenced patches and `patches`). When the lockfile check
  cannot run, it reports that once and still checks the files.

### The toolchain scan

* One scan, shared by the toolchain rules, reads the version files (`.nvmrc`, `.node-version`, `.tool-versions`,
  `mise.toml`, `.mise.toml`), Dockerfiles, `*.sh` files, the root's GitHub workflows and the root's Bazel module and
  workspace files, at the root and in every package directory `ignore-packages` does not ignore. `files` adds globs,
  `ignore-files` (strings or `{ name, reason }`) removes files, and `kinds` (`version-files`, `dockerfiles`,
  `workflows`, `shell-scripts`, `bazel`) picks what is read at the usual places.
* A file belongs to the innermost package that holds it; a file below a directory with a `package.json` that is not a
  workspace package belongs to another project and is not read. Directories are listed through
  `LintContext::listing_fs` ([ADR 0014](0014-files-the-js-analysis-reads-walk-git-index-fixtures.md)), and files are
  read through `LintContext::fs`.
* Files are read by line, with continuation lines, and commands are split by the shell tokenizer
  `skif_core::shell`, which the `package.json` script parser shares, so the rules do not depend on the oxc crates.

### `toolchain-pin-drift`

* `toolchain-pin-drift` (workspace, warning, no fix) reads by default version files, Dockerfiles, workflows and the
  Bazel files. A pin is a literal version in a version file, a `FROM node` tag, a `*_VERSION` `ARG` or `ENV`, a version
  input of the setup actions, a `corepack prepare|use|install` or a global npm install of a manager; a computed value or
  an alias such as `lts/*` is no pin.
* The Node source of truth is the first root version file that pins Node, in the order above, else the root's
  `engines.node`, which is also checked against a version file that is the source. The manager's source is
  `packageManager`, else `devEngines.packageManager`. Without a source nothing is compared.
* Both sides are node-semver ranges, and they drift when no version satisfies both. The finding points at the pin, with
  the source as a related location; the repair differs per tool, so there is no fix.
* A Dockerfile that runs `corepack enable` and no prepare gets one `CorepackUnpinned` finding at `info`, unless the
  rule's level is configured.

### `foreign-package-manager-command`

* `foreign-package-manager-command` (workspace, warning, no fix) reads by default Dockerfiles (`RUN`, not `CMD` or
  `ENTRYPOINT`), workflow `run:` steps and shell scripts, including command substitutions, and reports install, run and
  exec commands of a manager other than the detected one. `bun run` and `bun <file>` are not reported, since Bun is
  also a runtime.
* Exempt: `--version` and `--help`, read-only subcommands, a global install of only the chosen manager or corepack, a
  runner that runs the chosen manager, and a computed program. Without a detected manager nothing is reported. The
  flags of the managers differ, so there is no fix.

### Suppressions in toolchain files

`# skif-ignore-next-line [rule, …] -- <reason>` suppresses the named toolchain rules (all of them without names) on
the next command line, and `# skif-ignore-file [rule, …] -- <reason>` in the whole file. The reason is required: a
comment without one suppresses nothing, and the finding's help says so. `[[ignore]]` entries apply by the package a
finding names, and `[[overrides]]` by its package or file.

### Consequences

* Good, because a transitive ban, a family split and an unapplied patch fail the lint where the lockfile shows them,
  with the chain that leads there.
* Good, because a member's leftover lockfile and a member that silently forms its own workspace are reported under
  every supported manager, while a lockfile a build installs from is not.
* Good, because a pin in an image, a workflow or a version file is checked against the root, and a workflow still
  running another manager is reported where the call is.
* Good, because an exception is recorded with its reason and an exception that no longer applies is reported.
* Bad, because only pnpm format 9 and `bun.lock` are modelled; a Yarn or npm workspace gets a diagnostic that nothing
  was checked.
* Bad, because a consumer of a lockfile written another way (a computed label, a genrule command, a build script) is
  not seen and needs an `allow` entry.
* Bad, because files are read by line rather than by a Dockerfile or YAML parser, so flow mappings and heredoc `RUN`
  bodies are not understood.
* Bad, because the toolchain scan lists the package directories on every run, also when the JS analysis is off.

### Confirmation

* `crates/skif-workspace/src/lockfile.rs`: `a_lockfile_that_is_not_format_9_or_not_yaml_is_an_error`,
  `a_pnpm_10_patch_entry_is_the_hash_alone`, `the_project_document_of_a_two_document_lockfile_is_read_in_either_order`,
  `a_lockfile_without_exactly_one_project_document_is_an_error`, `every_document_must_be_format_9`,
  `the_environment_document_is_told_by_the_fields_of_its_importers` and `line_readers_get_the_project_document`;
  `crates/skif-js/src/lockfile.rs`: `a_two_document_pnpm_lock_is_read_from_the_project_document`;
  `crates/skif-cli/tests/cli.rs`: `a_lockfile_ban_reads_the_project_document_of_a_two_document_pnpm_lock`;
  `crates/skif-workspace/src/bun_lock.rs`: `importers_reach_the_copies_bun_installs_for_them`.
* `crates/skif-rules/src/policy/lockfile_bans.rs`: `a_missing_or_foreign_lockfile_is_reported_instead_of_passing`.
* `crates/skif-config/tests/config.rs`: `lockfile_scopes_are_read_and_checked`; `crates/skif-lint/tests/run.rs`:
  `rules_read_through_the_file_system_the_host_passes`.
* `crates/skif-rules/src/manifest/stray_lockfile.rs`: `an_npm_lockfile_next_to_the_shrinkwrap_is_reported`,
  `a_glob_entry_that_matches_no_stray_lockfile_is_stale`, `an_allow_entry_that_names_no_stray_lockfile_is_stale` and
  the `under_git_discovery_*` tests; `a_build_file_of_the_lockfiles_directory_names_it`,
  `a_dockerfile_that_copies_its_context_then_installs_names_the_lockfile` and
  `a_dockerfile_elsewhere_built_from_the_members_directory_names_its_lockfile`;
  `crates/skif-rules/src/toolchain/docker_builds.rs`: `build_commands_give_their_dockerfile_and_context` and
  `compose_services_give_their_build_contexts`.
* `crates/skif-rules/src/manifest/stale_patched_dependencies.rs`: `a_missing_patch_file_is_reported`,
  `a_patch_outside_the_allowed_paths_is_reported`, `a_patch_file_no_entry_references_is_reported` and
  `a_bun_patch_of_a_release_bun_lock_does_not_resolve_or_does_not_patch_is_reported_at_its_key`.
* `crates/skif-rules/src/toolchain/pin_drift.rs`: `exact_and_range_node_pins_are_checked_against_the_root_version_file`
  and `engines_node_must_admit_the_root_version_file`; `crates/skif-rules/src/toolchain/commands.rs`:
  `invocations_of_another_manager_are_found` and `the_chosen_manager_decides_what_is_foreign`;
  `crates/skif-rules/src/toolchain/parse.rs`: `dockerfile_copies_from_the_build_context_are_recorded`.
* `crates/skif-workspace/tests/memory.rs`: `package_manager_files_of_the_root_and_the_selected_directories_are_listed`
  and `ignored_package_manager_files_are_not_listed`.

## Pros and Cons of the Options

### Rules over a lockfile model read on demand and over one shared scan of toolchain files, with allow-lists and comment suppressions that carry reasons

* Good, because each finding has its own rule ID and default, and a run that asks for no lockfile check reads none.
* Bad, because skif reads file kinds it does not fully parse.

### Extend the manifest rules to these file kinds

* Good, because no further rule IDs appear.
* Bad, because one rule ID would cover findings with different causes, defaults and fixes, and the manifest rules would
  depend on the lockfile.

### Leave lockfiles and toolchain files to dedicated tools

* Good, because skif stays a manifest and source linter.
* Bad, because no common tool checks these against the workspace's own manager, catalogs and policies, which skif
  already knows.
