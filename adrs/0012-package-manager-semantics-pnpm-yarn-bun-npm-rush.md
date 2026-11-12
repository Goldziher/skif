---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Package-Manager Semantics: pnpm, Yarn, Bun, npm and Rush

## Context and Problem Statement

The same `package.json` files mean different installs under different package managers. Bun expands workspace
patterns differently from the others and allows members outside the root. A workspace member's binary reaches every
package when the manager links all member binaries into the root `node_modules/.bin`, and only the packages that list
the member when it installs each package in isolation. Which layout a repository gets depends on the manager, its
version and settings in `pnpm-workspace.yaml`, `.npmrc`, `bunfig.toml` or `bun.lock`. Some repositories hoist chosen
packages to the root on purpose and import them without listing them. Treating every workspace as npm reports
binaries the install provides, misses ones it does not, lints members Bun never installs and judges patches against
the wrong lockfile.

How does skif decide what the detected package manager installs, and which repository files does it trust for that?

## Decision Drivers

* An answer follows what the detected manager does with the committed files
* Only files committed to the repository decide; user, environment and command-line settings are not seen
* Every other manager keeps its answer when a rule follows one manager's behaviour
* knip's answer is the default where skif and the manager would otherwise disagree with knip
* Rush repositories behave as Rush installs them, whatever their pnpm settings

## Considered Options

* Follow the detected manager, keyed on the repository's committed files, with public hoisting as an opt-in
* One npm-like model for every manager
* Ask the user to declare the layout in `skif.toml`

## Decision Outcome

Chosen option: "Follow the detected manager, keyed on the repository's committed files, with public hoisting as an
opt-in", because each difference is a false or missed finding the repository's files settle, and an opt-in keeps
knip's answer where a hoist would otherwise stand in for a listing.

### Detection

* Manager detection is that of workspace discovery ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)).
  Its last step is a root `bunfig.toml` with an `[install]` table, which configures Bun's installer and means Bun; a
  `bunfig.toml` that only configures the runtime or `bun test`, or that does not parse, decides nothing.
* Yarn is Berry when the root's `packageManager` names Yarn 2 or later or, without one, when the root has a
  `.yarnrc.yml` or a Berry `yarn.lock`; a Yarn whose version none of these tells counts as hoisting, like an unknown
  manager.
* Under pnpm the lockfile is `pnpm-lock.yaml` in lockfile format 9, including the two-document file of pnpm 11 and
  later, whose package-manager environment document is ignored and whose project document is read
  ([ADR 0011](0011-lockfile-and-toolchain-rules.md)).

### Bun workspaces

* A positive pattern without glob characters names a member that no negation removes.
* A positive pattern may start with `..` segments and selects members outside the root. They count for the pattern's
  matches and are recorded in `DiscoveryFacts::outside_root_members`, but they are not loaded: everything skif lints
  lies below the root. A `..` anywhere else, and under any other manager, is invalid.
* A member without a `name` is recorded in `DiscoveryFacts::nameless_members`, excluded from analysis, and reported by
  `missing-package-manifest` with `detail` `NamelessBunMember`; the unsafe fix names it after its directory.
* `bun.lock` is the workspace's lockfile ([ADR 0011](0011-lockfile-and-toolchain-rules.md)): `BunLockfile` resolves a
  dependency as Bun looks it up, below the package's own key, then below each key it is nested in, then at the top.
  Findings name the release, not the key.
* The `bun` plugin reads `bunfig.toml`: `[test] root` and `pathIgnorePatterns` replace the default test globs,
  `preload` and `[lint] plugins` name dependencies and entries, and `Bun.build` `entrypoints` computed from the calling
  file's location resolve from that file.
* `resolutions` next to `overrides` is not reported, although Bun ignores `resolutions` then; both are read as policy
  instances.

### Member binaries

A workspace member's binary reaches a package that does not list the member only when the manager links every
member's binaries into the root:

* npm, unless the last top-level `install-strategy` of the root `.npmrc` is `linked`;
* Yarn Classic;
* pnpm with the `hoisted` node linker;
* Bun with the hoisted linker;
* an unknown manager.

Under pnpm's `isolated` (the default) and `pnp` linkers, Yarn Berry, Bun's isolated linker, npm's `linked` strategy
and in a Rush repository (a `rush.json` at the root, whatever the manager), a member's binary reaches only the packages
that list the member, the packages below one that does, and the member's own scripts.

### Linker settings

* pnpm: `nodeLinker` of `pnpm-workspace.yaml` when it is a string; otherwise the last top-level `node-linker` of the
  root `.npmrc`, but only when `packageManager` names a pnpm before 11 or no pnpm version, since pnpm 11 reads layout
  settings only from `pnpm-workspace.yaml`. Without a setting the linker is `isolated`.
* Bun: `linker` of `[install]` in the root `bunfig.toml`; otherwise a `bun.lock` that records `configVersion` 1 means
  isolated and one without it hoisted. With neither file Bun counts as hoisting.
* `.npmrc` is read as npm's `ini` parser reads it: trimmed, unquoted, without a byte order mark or an inline comment,
  nothing under a `[section]`. A member's `.npmrc`, the user's `.npmrc`, `npm_config_*` variables and command-line
  flags are not read.

### External binaries and hoisting

* An external binary is credited only to a package that, itself or through an ancestor, lists the dependency that
  provides it, never through another package's dependency, under every manager.
* `[js] use-public-hoisting` (default `false`, top level only) lets packages that the install hoists on purpose satisfy
  an import, or a package a tool config names, that no manifest of the package or above lists. Under pnpm (not Rush)
  `publicHoistPattern` and `shamefullyHoist` of `pnpm-workspace.yaml` decide, else `public-hoist-pattern` and
  `shamefully-hoist` of the root `.npmrc` before pnpm 11, matched as pnpm's matcher does; without a setting nothing is
  hoisted. In a Bazel repository the `public_hoist_packages` dict literals of the root module or workspace file hoist
  into the Bazel packages they name ([ADR 0031](0031-bazel.md)).
* Hoisting satisfies an unlisted import and makes nothing used or unused; a listed package is found through its
  listing first. Binaries are not affected. `node-linker=hoisted` and the flat npm, Yarn Classic and Bun layouts are
  not counted as hoisting, since they would turn `unlisted-dependencies` off, which a rule level already does.
* The setting is ignored under workspace isolation, `[js] isolate-workspaces` and strict mode, which implies it: a hoist
  is a listing of the root ([ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md)).

### Rush

`rush`, `rushx` and `rush-pnpm` are provided commands in a Rush repository, and a Rush repository is always isolated.

### Consequences

* Good, because a Bun workspace's members, lockfile, nested workspaces, patches and test roots match what `bun install`
  and `bun test` do, and every other manager keeps its answers.
* Good, because a pnpm workspace with the hoisted linker, or an npm workspace with the linked strategy, gets the
  member-binary answer its install gives.
* Good, because a repository that hoists packages on purpose states it once and keeps `unlisted-dependencies` on for
  everything else, while the default and strict mode keep knip's answer.
* Bad, because a linker or hoist set only in the user's `.npmrc`, the environment, the command line or a `.bzl` file is
  not seen.
* Bad, because a pnpm 11 workspace that declares no pnpm version and keeps a stale `.npmrc` setting is read as pnpm 10.
* Bad, because Bun members outside the root are not linted; their directory has to be linted as a root of its own.
* Bad, because a hoist pattern can satisfy the import of a package the repository never installs.

### Confirmation

* `crates/skif-js/tests/monorepo_regressions.rs`: `a_member_binary_is_provided_where_the_package_manager_links_it`,
  `pnpm_links_member_binaries_as_its_node_linker_says`, `bun_links_member_binaries_as_its_linker_says`,
  `npm_links_member_binaries_as_its_install_strategy_says`, `yarn_is_berry_only_on_evidence` and
  `rush_commands_are_provided_in_a_rush_repository`.
* `crates/skif-workspace/src/pnpm.rs`: `node_linker_is_read_when_it_is_a_string` and
  `public_hoisting_settings_are_read_when_well_typed`; `crates/skif-cli/tests/cli.rs`:
  `a_lockfile_ban_reads_the_project_document_of_a_two_document_pnpm_lock`.
* `crates/skif-workspace/src/roots.rs`: `declared_member_dirs_keeps_a_literal_member_a_negation_names_only_under_bun`;
  `crates/skif-workspace/tests/memory.rs`: `bun_selects_members_outside_the_root_without_loading_them`;
  `crates/skif-workspace/tests/discovery.rs`:
  `bun_selects_the_members_bun_install_links_and_reads_the_workspaces_catalogs`;
  `crates/skif-rules/src/manifest/unmatched_workspace_patterns.rs`: `a_member_outside_the_root_is_valid_under_bun`.
* `crates/skif-js-plugins/tests/plugins.rs`: `bun_test_root_replaces_the_default_test_globs`,
  `bun_test_path_ignore_patterns_leave_test_files_out` and `bun_reads_the_lint_plugins_from_bunfig`.
* `crates/skif-js/tests/public_hoisting.rs`: `pnpm_public_hoist_settings_make_packages_available_when_on`,
  `bazel_public_hoist_packages_make_packages_available_where_they_are_hoisted`,
  `hoisting_leaves_unused_dependencies_as_they_are` and `strict_mode_does_not_count_hoisted_packages`.

## Pros and Cons of the Options

### Follow the detected manager, keyed on the repository's committed files, with public hoisting as an opt-in

* Good, because each answer is the one the install gives for the committed files.
* Bad, because answers depend on detection, and a repository detected as another manager keeps that manager's answer.

### One npm-like model for every manager

* Good, because it is simple and the same everywhere.
* Bad, because it reports binaries isolated installs do not provide as provided, lints members Bun never installs, and
  checks patches against the wrong lockfile.

### Ask the user to declare the layout in `skif.toml`

* Good, because nothing has to be inferred.
* Bad, because the layout is already declared in the manager's own files, and a second declaration drifts from them.
