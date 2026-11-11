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
ancestors' dependencies, or `[js.plugins] nx = true`): every `project.json` below the package directory, outside
`node_modules`. `skif-js` leaves out those the file walk leaves out: below a gitignored directory (unless
`--no-gitignore`) or below a directory the walk skips by name (build output at the top of the package, hidden
directories). `skif_js_plugins::NxProjects` holds the result. `NxProjects::find` finds them and `NxProjects::filter`
narrows them. `resolve_plugins_with(ctx, &projects)` runs the plugins with them, and `resolve_plugins` finds them
itself.

* The `nx` plugin reads every project's `project.json` from the package, in addition to `nx.json` and the package's
  own `project.json`. Nx runs a target's commands in the workspace root, so the commands and paths a target holds are
  relative to the package directory. This replaces the `{apps,libs}/**/project.json` glob.
* A project directory with no `package.json` in it, or in a directory between it and the package directory, is a
  **scope**. Every other enabled plugin also runs there, with a context whose directory is the scope and whose
  dependencies are the package's, without the package's scripts and with only the switches of `[js.plugins]` (a
  table's globs are relative to the package). What the plugin finds there is rebased onto the package and merged into
  its output for the package. Globs and paths gain the scope as a prefix, and an entry expression matches below the
  scope only. The config files are added. The packages the configs name are the package's to list, as for its own
  configs. A config of a scope that hides entries anywhere (`IncompleteScope::Package`) hides them in the scope
  (`<scope>/**`), not in the whole package; a narrower scope is rebased.
* A project with a `package.json` of its own is that package's project: as a workspace member it is a package and
  runs its plugins itself. A buildable library whose `package.json` is not a member stays outside the analysis, as
  every directory with a manifest of its own does.

### Target options and tokens

In a target, `{workspaceRoot}` stands for the workspace root and `{projectRoot}` for the project's directory, both
relative to the package. `main` and `additionalEntryPoints` are production entries, and `tsConfig` and `jestConfig` are
configs of the `typescript` and `jest` plugins. All of them are relative to the workspace root. The `cwd` of
`run-commands`, relative to the workspace root, is where its commands run, so they are read as `cd <cwd> && <command>`.
A path with another token (`{options.outputPath}`), an absolute one or one outside the package is not read. The
configs a target links are another package's when they lie in one, so their aliases and output directories stay with
that package, as for a `tsconfig` reference.

### `getJestProjects()`

A Jest config whose `projects` is not known statically is incomplete, unless the source imports `getJestProjects` or
`getJestProjectsAsync` from `@nx/jest` or `@nrwl/jest` and `projects` is not overridden by a later spread. Nx returns the
Jest configs of its projects, so these are read as the config's projects. Each is the `jestConfig` option of a
project's target or a `jest.config.*` in a project directory. The evaluator keeps no trace of where an unknown value
came from, so the call is recognised in the source.

### Consequences

* Good, because an integrated workspace's tool configs are read: the configs, the entries they give (the scripts of
  `index.html`, test files, a Jest `preset`) and the packages they name are no longer reported.
* Good, because the root `jest.config.ts` of an Nx workspace no longer turns off the unused-code rules of the root.
* Good, because a computed config in one project leaves only that project unjudged.
* Good, because a `project.json` under `packages/`, `tools/` or any other directory is read, with its tokens, target
  options and `cwd`.
* Bad, because each scope runs the plugin table once more. Most configs are looked for at the top of the scope, but
  a plugin that walks its directory walks the scope again after the package's own walk.
* Bad, because the `jest` plugin finds `getJestProjects()` projects with its own walk, which does not read
  `.gitignore` files, so a gitignored project's Jest config is still read there.
* Neutral, because a project with its own `package.json` that is not a workspace member keeps the files below it out
  of the analysis.

### Confirmation

* `crates/skif-js-plugins/tests/nx_projects.rs`: a scope's Vite, ESLint and Jest configs are read for the root, with the
  `index.html` script, the Jest `preset` and an anchored `testRegex` rebased. A `project.json` makes no scope without
  `nx`, and none with a `package.json` of its own or above it. Every `project.json` is read, except below
  `node_modules` and those the caller filters out. A computed config of a scope is bounded by `<scope>/**`.
* `crates/skif-js-plugins/src/scopes.rs` tests: glob and expression rebasing, including flags, alternations and a
  sibling directory with the same prefix.
* `crates/skif-js-plugins/tests/plugins.rs` (`nx_target_options_tokens_and_cwd`): `main`, `additionalEntryPoints`,
  `tsConfig`, `jestConfig`, both tokens and `cwd`.
* `crates/skif-js-plugins/tests/projects.rs`: `getJestProjectsAsync()` and `getJestProjects()` read the projects' Jest
  configs and leave the root complete; another function, or Nx's name imported from elsewhere, stays incomplete.
* `crates/skif-js/tests/nx_integrated.rs`: an integrated workspace reports only its one unused file and unused
  dependency, with and without the root `jest.config.ts`; without `nx` its configs are unused files.
* `crates/skif-js/src/discover.rs` tests: the walk's rules for the `project.json` files skif-js keeps.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Each project directory without a `package.json` is a scope of the package that owns it

* Good, because every plugin's conventions apply to the project: default entries, `index.html`, configs found by
  name, `resolve_without_config`.
* Good, because the findings and the dependencies stay with the manifest that lists them.
* Bad, because the plugin output needs rebasing, and an entry expression that matches from its start may also match a
  sibling directory whose name starts with the scope's.

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
