---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
---

# Nx Projects as Scopes of the Package That Owns Them

## Context and Problem Statement

An Nx project is a directory with a `project.json`. In an integrated Nx workspace a project has no `package.json`:
the root manifest lists every dependency, and each project keeps its own tool configs (`vite.config.ts`,
`eslint.config.mjs`, `jest.config.ts`, `.babelrc`, `index.html`, `tsconfig.json`) in its directory. The plugins of
[ADR 0011](0011-plugin-model.md) run once per package with config globs relative to the package directory, so these
configs were never read. A fresh integrated React workspace got about twenty false findings: the configs and
`src/main.tsx` as unused files, and `@vitejs/plugin-react`, the ESLint plugins, the Babel presets, `jest`, `ts-jest` and
`ts-node` as unused dependencies.

Nx's root `jest.config.ts` (`projects: await getJestProjectsAsync()` from `@nx/jest`) made it worse. The value is not
known statically, so the root package was incomplete as a whole (ADR 0065), and an integrated workspace, being one
package, got no unused file, export or dependency at all. The `nx` plugin also read `project.json` only at the
package root and under `apps/` and `libs/`, ignored the paths that target options name, and read
`{projectRoot}` and `{workspaceRoot}` as literal text.

How do the plugins see a project that is not a package, and what does `getJestProjects()` return?

## Decision Drivers

* An integrated project's configs are read as Nx reads them: relative to the project directory, with the root's
  dependencies
* No new configuration: the projects come from what Nx itself reads
* Package-based Nx workspaces, where each project is a workspace member, keep their reports
* Library crates do no I/O beyond the `FileSystem` trait, and the walk rules (`.gitignore`, skipped directories) stay
  in one place

## Considered Options

* Each project directory without a `package.json` is a scope of the package that owns it
* Queue the configs found in project directories as further configs of the package's plugins
* Treat each project as a workspace package of its own

## Decision Outcome

Chosen option: "Each project directory without a `package.json` is a scope of the package that owns it", because the
plugins then read a project exactly as they read a package, including their defaults and conventions, while the
findings stay with the package whose manifest lists the dependencies.

### Projects and scopes

The projects are found when the `nx` plugin is enabled for the package (`nx`, `@nx/*` or `@nrwl/*` among its or its
ancestors' dependencies, or `[js.plugins] nx = true`). `skif-js` lists every `project.json` once per run, with the
rules of its source walk: below no gitignored directory (unless `--no-gitignore`), hidden directory, `node_modules`,
build output skipped by name or directory of a `package.json` that is no workspace member. Each project is owned by
the package whose directory is closest above it. `skif_js_plugins::NxProjects` holds what a package sees: its path in
the Nx workspace, every project below it (`all`), the ones it owns (`dirs`) and those of them without a `package.json`
(`scopes`). `NxProjects::from_dirs` builds it from such a listing, and `resolve_plugins_with(ctx, &projects)` runs the
plugins with it, which also hands it to the plugins as `PluginContext::nx`. `resolve_plugins` finds the projects
itself (`NxProjects::find`), walking the package without the `.gitignore` and build-output rules and taking the
package for the Nx workspace root.

* The `nx` plugin reads the `project.json` of the package and of each of its scopes, in addition to `nx.json`. A
  workspace member reads its own; the root does not read a member's, so a member's executors and paths count for the
  member. This replaces the `{apps,libs}/**/project.json` glob.
* A project directory with no `package.json` in it, or in a directory between it and the package directory, is a
  **scope**. Every other enabled plugin also runs there, with a context whose directory is the scope and whose
  dependencies are the package's, without the package's scripts and with only the switches of `[js.plugins]` (a
  table's globs are relative to the package). What the plugin finds there is rebased onto the package and merged into
  its output for the package. Globs and paths gain the scope as a prefix. An entry expression keeps its text and is
  stored with its scope (`ScopedEntryRegex`): it is matched against the path below the scope, for the files there
  only, as the `regex` crate has no look-around to bound a rebased expression to the scope. The config files are
  added. The packages the configs name are the package's to list, as for its own configs. A config of a scope that
  hides entries anywhere (`IncompleteScope::Package`) hides them in the scope (`<scope>/**`), not in the whole
  package; a narrower scope is rebased.
* What enables a plugin in a scope is decided once for all of a package's scopes, as they share its dependencies and
  switches; only a file enabler (`Enabler::File`) looks at each scope. Each scope's files are listed once, and the
  plugins' config globs and file enablers are matched against that listing. A plugin with no default entry,
  production, project or ignore globs that finds no config file in a scope is not run there, so its config-less and
  workspace conventions (Vite's `index.html` without a `vite.config.*`) do not apply to the scope.
* A project with a `package.json` of its own is that package's project: as a workspace member it is a package and
  runs its plugins itself. A buildable library whose `package.json` is not a member stays outside the analysis, as
  every directory with a manifest of its own does.

### Target options and tokens

Nx gives the paths of a target relative to the workspace root, so they are read relative to it and then made relative
to the package that reads the `project.json`; a path outside that package is not read. `{workspaceRoot}` stands for
the workspace root and `{projectRoot}` for the project's directory relative to it, as Nx replaces it. `main` and
`additionalEntryPoints` are production entries. `tsConfig`, `jestConfig`, `webpackConfig`, `cypressConfig` and
`eslintConfig` are configs of the `typescript`, `jest`, `webpack`, `cypress` and `eslint` plugins. `configFile`,
`config` and `configDir` are configs of the `vite`, `playwright` and `storybook` plugins only for the executors of
`@nx/vite`, `@nx/playwright` and `@nx/storybook` (or `@nrwl/*`), as other executors use these names otherwise;
`configDir` names the directory of Storybook's `main` file. The commands of `run-commands` run in the workspace root,
or in its `cwd` relative to it, so they are read as `cd <that directory, from the package> && <command>`, with
`{workspaceRoot}` as the way back to the root. A file such a command names through the root
(`cd ../.. && node packages/b/run.mjs` in `packages/b`) is the package's own (`run.mjs`). A path with another token
(`{options.outputPath}`) or an absolute one is not read. The configs a target links are another package's when they
lie in one, so their aliases and output directories stay with that package, as for a `tsconfig` reference.

### `getJestProjects()`

A Jest config whose `projects` is not known statically is incomplete, unless the source imports `getJestProjects` or
`getJestProjectsAsync` from `@nx/jest` or `@nrwl/jest`, `projects` is not overridden by a later spread, and it is the
call itself or an array with no more unknown elements than the source has calls (`[...getJestProjects(),
'<rootDir>/tools']`). Nx returns the Jest configs of its projects, so these are read as the config's projects, besides
the array's known elements. Each is the `jestConfig` option of a project's target or a `jest.config.*` in a project
directory, for every project below the package (`NxProjects::all`, or a walk without the `nx` plugin). The evaluator
keeps no trace of where an unknown value came from, so the call is recognised in the source.

### Consequences

* Good, because an integrated workspace's tool configs are read: the configs, the entries they give (the scripts of
  `index.html`, test files, a Jest `preset`) and the packages they name are no longer reported.
* Good, because the root `jest.config.ts` of an Nx workspace no longer turns off the unused-code rules of the root.
* Good, because a computed config in one project leaves only that project unjudged.
* Good, because a `project.json` under `packages/`, `tools/` or any other directory is read, with its tokens, target
  options and `cwd`, and a member's paths are read from the workspace root as Nx reads them.
* Good, because one walk lists the projects of every package of a run, and each scope's files are listed once for all
  plugins.
* Bad, because each scope runs the plugins its files call for once more, and a plugin that walks its directory walks
  the scope again.
* Bad, because a plugin that has neither a config file nor default globs in a scope does not apply its config-less
  conventions there.
* Bad, because `resolve_plugins` without a caller's listing, and the `jest` plugin in a scope, find projects with a
  walk that does not read `.gitignore` files or skip build output.
* Neutral, because a project with its own `package.json` that is not a workspace member keeps the files below it out
  of the analysis.

### Confirmation

* `crates/skif-js-plugins/tests/nx_projects.rs`: a scope's Vite, ESLint and Jest configs are read for the root, with the
  `index.html` script, the Jest `preset` and a scoped `testRegex`. A `project.json` makes no scope without `nx`, and
  none with a `package.json` of its own or above it. The root reads its own and its scopes' `project.json`, not a
  member's, and a member reads its own with paths, `cwd` and tokens relative to the workspace root. A computed config
  of a scope is bounded by `<scope>/**`. A scope runs only the plugins its files call for.
* `crates/skif-js-plugins/src/scopes.rs` tests: glob rebasing, entry expressions kept with their scope, and paths
  through the workspace root.
* `crates/skif-js/src/globs.rs` tests: a scope's expressions match below the scope only, not in a sibling directory
  whose name starts with the scope's.
* `crates/skif-js-plugins/src/files.rs` tests: a listing searched for globs finds what a walk finds.
* `crates/skif-js-plugins/tests/plugins.rs` (`nx_target_options_tokens_and_cwd`,
  `nx_target_options_link_the_configs_of_other_tools`): `main`, `additionalEntryPoints`, the linked config options,
  both tokens and `cwd`.
* `crates/skif-js-plugins/tests/projects.rs`: `getJestProjectsAsync()` and `getJestProjects()`, also spread into an
  array, read the projects' Jest configs and leave the root complete; another function, Nx's name imported from
  elsewhere or a further unknown element stays incomplete.
* `crates/skif-js/tests/nx_integrated.rs`: an integrated workspace reports only its one unused file and unused
  dependency, with and without the root `jest.config.ts`; without `nx` its configs are unused files. A member's
  `project.json` makes its build entry and scripts entries of the member.
* `crates/skif-js/src/discover.rs` and `input.rs` tests: the walk's rules for `project.json` files and their owners,
  and one walk per run.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Each project directory without a `package.json` is a scope of the package that owns it

* Good, because every plugin's conventions apply to the project: default entries, `index.html`, configs found by
  name, `resolve_without_config`.
* Good, because the findings and the dependencies stay with the manifest that lists them.
* Bad, because the plugin output needs rebasing, and an entry expression cannot be rebased in its text: it is kept
  with its scope.

### Queue the configs found in project directories as further configs of the package's plugins

* Good, because no output is rebased: each config is read with its directory as its base.
* Bad, because only configs are read. A plugin's default entries, the files it finds without a config and its
  config-less conventions do not apply to the project, and each plugin's config globs would have to be repeated for
  every project.

### Treat each project as a workspace package of its own

* Good, because the module graph and the rules already handle packages.
* Bad, because a project has no manifest, so every dependency its configs or files use would be unlisted there and
  unused at the root, the opposite of how Nx installs them.

## More Information

* Nx project configuration: <https://nx.dev/reference/project-configuration>
* `nx:run-commands` options: <https://nx.dev/nx-api/nx/executors/run-commands>
* `getJestProjectsAsync`: <https://nx.dev/technologies/test-tools/jest/api>
