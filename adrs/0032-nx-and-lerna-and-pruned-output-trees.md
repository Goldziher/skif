---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Nx and Lerna (and Pruned Output Trees): Projects as Scopes, Lerna as Facts, Layouts Recognised by Shape

## Context and Problem Statement

An Nx project is a directory with a `project.json`. In an integrated Nx workspace a project has no `package.json`:
the root manifest lists every dependency, and each project keeps its own tool configs in its directory, while the
plugins run once per package with globs relative to the package ([ADR 0015](0015-plugin-model-and-plugin-set.md)).
Nx target options name entries and configs relative to the workspace root, with `{projectRoot}` and
`{workspaceRoot}` tokens; local Nx plugins and executors are modules of the repository; and Nx's root
`jest.config.ts` computes its projects with `getJestProjects()`.

Lerna reads its package list from `lerna.json` before the package manager's declaration, has removed `useWorkspaces`
and several commands in its 7 and 9 majors, and fails on configuration skif did not report. Repositories built with
these tools also hold layouts that are there by design: task-ordering dependencies on members with no code, grouping
directories and Nx projects without a manifest, and `turbo prune` output next to a pruned lockfile.

How do the plugins see an Nx project that is not a package, what does skif read and report of Lerna, and which
layouts does it recognise without configuration?

## Decision Drivers

* An integrated project's configs are read as Nx reads them: relative to the project, with the root's dependencies
* No new configuration: projects come from what Nx itself reads
* Package-based workspaces, where each project is a member, keep their reports
* The package manager's declaration stays the one source of members
* A configuration the installed Lerna rejects is reported before a release breaks on it
* Each layout is decided by files skif already reads

## Considered Options

* Nx project directories without a `package.json` are scopes of the package that owns them; Lerna is read as facts
  with its own rules; task-ordering dependencies, grouping directories and prune output are recognised by shape
* Treat each Nx project as a workspace package of its own
* Let `lerna.json` declare the members whenever it exists
* Leave these layouts to `ignore` entries

## Decision Outcome

Chosen option: "Nx project directories without a `package.json` are scopes of the package that owns them; Lerna is
read as facts with its own rules; task-ordering dependencies, grouping directories and prune output are recognised by
shape", because the plugins then read a project exactly as they read a package while the findings stay with the
package whose manifest lists the dependencies, and every other case is decided without a setting.

### Nx projects and scopes

* Projects are found only when the `nx` plugin is enabled for the package. Every `project.json` is listed once per
  run, with the source walk's rules ([ADR 0014](0014-files-the-js-analysis-reads-walk-git-index-fixtures.md)), and
  owned by the closest package above it (`NxProjects`).
* The `nx` plugin reads the `project.json` of the package and of its scopes, besides `nx.json`. A workspace member
  reads its own; the root does not read a member's.
* A project directory with no `package.json` in it or between it and its package is a scope. Every other enabled
  plugin also runs there, with the scope as its directory and the package's dependencies; its output is rebased onto
  the package. An entry expression is stored with its scope and matched only below it. A config of a scope that hides
  entries everywhere hides them only in `<scope>/**` ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)).
* A plugin with no default globs that finds no config file in a scope runs there only when a target of the scope's
  `project.json` runs its Nx executor (`@nx/<id>:*` or `@nrwl/<id>:*`, or a known other name), and then applies its
  config-less conventions.
* A project with its own `package.json` is that package's: a member runs its plugins itself, and a non-member stays
  outside the analysis.

### Nx target options

* Paths are read relative to the workspace root, then made relative to the package that reads the `project.json`.
  `{workspaceRoot}` and `{projectRoot}` are substituted as Nx substitutes them; another token or an absolute path is
  not read.
* `main` and `additionalEntryPoints` are production entries. The `*Config` options (`tsConfig`, `jestConfig`,
  `webpackConfig`, `cypressConfig`, `eslintConfig`) are configs of their plugins; `configFile`, `config` and
  `configDir` are only for the executors of the matching Nx plugin.
* `run-commands` commands are read as `cd <cwd> && <command>` from the package
  ([ADR 0017](0017-scripts-binaries-and-working-directories.md)).
* A Jest config whose `projects` is a `getJestProjects()` or `getJestProjectsAsync()` call imported from Nx (alone or
  spread into an array) resolves to every project's Jest config instead of making the package incomplete. A Cypress
  config that spreads `nxE2EPreset()` gets the preset's support file and spec pattern. Both calls are recognised in
  the source.

### Local Nx plugins and executors

* A plugin of `nx.json`, an executor, a `targetDefaults` key or a `generators` key names a local module when its
  specifier is a path relative to the workspace root, or matches a `paths` pattern of the root `tsconfig.base.json`
  (else `tsconfig.json`) whose target exists. `extends` is not followed.
* A local plugin's module is an entry. Every `implementation`, `batchImplementation` and `factory` in the
  `executors`, `builders`, `generators` and `schematics` collections of its `package.json` is an entry, because a
  collection is the plugin's public interface. A workspace package for which `nx` is enabled reads its own
  collections.

### Lerna

* Discovery reads a valid `lerna.json` beside the root `package.json` whenever it exists and records it as
  `DiscoveryFacts::lerna`: its `packages`, `useWorkspaces`, whether `version` is a string, and, when another file
  declares the members, the directories only one of the two lists selects. Its `packages` and `npmClient` decide the
  members and the manager only when nothing else declares them
  ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)).
* `stale-lerna-config` (workspace, warning): with Lerna 7 or later, `useWorkspaces` and a missing `version`; with
  Lerna 9 or later, a root or member script that runs `lerna bootstrap`, `add` or `link`, directly or through a
  package-manager runner (a pinned runner such as `npx lerna@6` runs that release and is not reported). The release
  comes from the installed `node_modules/lerna`, else the lowest version the root range admits; without one nothing
  is reported.
* `lerna-packages-mismatch` (workspace, warning) reports, at `packages` in `lerna.json`, when Lerna's list and the
  declared members select different packages, naming both. It is skipped under `useWorkspaces: true`.

### Layouts recognised by shape

* `unused-dependencies` and `unused-dev-dependencies` do not report a dependency on a member with none of `main`,
  `module`, `exports`, `bin`, `browser`, `types` and `typings` and no code file: nothing can import or run it, so the
  dependency orders tasks.
* `missing-package-manifest` skips a selected directory that holds a `project.json`, or whose content is only
  directories the patterns also select.
* The root walk of a recursive run does not enter the `json` and `full` directories that sit beside a lockfile in a
  directory below the start path with no `package.json`, the output of `turbo prune`. The start directory itself is
  never skipped, so `skif -r out` lints both trees.

### Consequences

* Good, because an integrated workspace's tool configs, the entries they give and the packages they name are read,
  and a computed config in one project leaves only that project unjudged.
* Good, because a local Nx plugin is not an unlisted dependency, and its executors and generators are analysed.
* Good, because a `lerna.json` the installed Lerna rejects, scripts that call removed commands and a Lerna list that
  disagrees with the workspaces are reported, while skif's members stay the ones the package manager installs.
* Good, because task-ordering dependencies, grouping directories, Nx projects and prune output produce no findings,
  without configuration.
* Bad, because each scope runs the plugins its files call for once more, and a plugin without a config or default
  globs applies its conventions only when an executor names it.
* Bad, because only the root tsconfig's own `paths` are read for local plugins.
* Bad, because a stale dependency on a member without a module entry and without code is not reported.
* Bad, because a Lerna installed outside the root `node_modules` and not declared at the root is not seen.

### Confirmation

* `crates/skif-js-plugins/tests/nx_projects.rs`: scope configs read for the root, no scope without `nx` or with a
  `package.json`, `<scope>/**` bounds, and `a_scope_whose_targets_run_a_tools_executor_applies_its_conventions`.
* `crates/skif-js-plugins/tests/plugins.rs`: `nx_target_options_tokens_and_cwd`,
  `nx_target_options_link_the_configs_of_other_tools`, `nx_local_plugins_executors_and_generators_are_followed`,
  `nx_executors_name_collections_not_modules` and `cypress_nx_e2e_preset`.
* `crates/skif-js-plugins/tests/projects.rs`: `nx_jest_projects_are_the_jest_configs_of_the_nx_projects` and
  `nx_jest_projects_spread_into_an_array`.
* `crates/skif-js/tests/nx_integrated.rs`: `the_tool_configs_of_integrated_projects_count_for_the_root` and
  `a_local_nx_plugin_named_by_an_alias_is_followed`.
* `crates/skif-workspace/src/lerna.rs` tests, and `crates/skif-rules/src/manifest/stale_lerna_config.rs` and
  `lerna_packages_mismatch.rs` tests (among them `lists_that_select_the_same_packages_are_clean`).
* `crates/skif-js/tests/task_ordering_dependencies.rs`:
  `dependencies_on_members_without_a_module_entry_or_source_are_not_reported`.
* `crates/skif-rules/src/manifest/missing_package_manifest.rs`:
  `grouping_directories_and_nx_projects_need_no_manifest`.
* `crates/skif-workspace/src/roots.rs`: `the_trees_of_turbo_prune_output_are_not_searched_for_roots` and
  `prune_output_given_as_the_start_directory_is_searched`.

## Pros and Cons of the Options

### Nx project directories without a `package.json` are scopes of the package that owns them; Lerna is read as facts with its own rules; task-ordering dependencies, grouping directories and prune output are recognised by shape

* Good, because the plugins, their defaults and their conventions apply to a project unchanged.
* Good, because one walk lists every project of a run, and each scope's files are listed once for all plugins.
* Bad, because the scope machinery (rebasing, scoped expressions) is a second way plugins see a directory.

### Treat each Nx project as a workspace package of its own

* Good, because no scope concept is needed.
* Bad, because a project lists no dependencies, so every package its configs name would be unlisted there and unused
  at the root.

### Let `lerna.json` declare the members whenever it exists

* Good, because skif would see what Lerna sees.
* Bad, because the package manager installs its own members, and Lerna's list would override the source of truth.

### Leave these layouts to `ignore` entries

* Good, because it needs no code.
* Bad, because every Nx, Lerna and Turborepo repository would restate its layout in `skif.toml`.
