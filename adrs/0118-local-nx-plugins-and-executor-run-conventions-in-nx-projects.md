---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0109"]
---

# Local Nx Plugins and Executor-Run Conventions in Nx Projects

## Context and Problem Statement

The `nx` plugin read every `nx.json` `plugins` entry, target `executor` and generator as a package name. Nx also
loads plugins, executors and generators from the workspace itself: by a path relative to the workspace root
(`./tools/nxp/index.ts`, `"executor": "./tools/executors/echo:echo"`) or by a `paths` alias of the root
`tsconfig.base.json` (`@fx/nxp` → `tools/nxp/index.ts`). An executor or generator then comes from the `executors`,
`builders`, `generators` or `schematics` collection that the local plugin's `package.json` names, whose members name
their module in `implementation`, `batchImplementation` or `factory`. skif reported such a plugin as an unlisted
dependency once per config that named it, reported its module as unused, and never analysed the executor
implementations.

[ADR 0109](0109-nx-projects-as-scopes-of-the-package-that-owns-them.md) runs the plugins in each Nx project scope, but
skips a plugin that has neither default globs nor a config file there, so that a scope does not run every plugin. An
Nx executor runs its tool without a config file of the project (`@nx/vite:build` with Vite's defaults), so the tool's
config-less conventions apply although no config exists: such a project's `index.html` names `src/main.tsx`, which was
reported as unused.

Nx's Cypress preset, `nxE2EPreset(__filename, options)` from `@nx/cypress/plugins/cypress-preset`, is spread into the
`e2e` section of every Nx e2e project's `cypress.config.ts`. Its value is not known statically, so the support files it
sets were reported as unused.

How does the `nx` plugin tell a local plugin from a package and follow it, which plugins run in a scope without a
config file, and what does `nxE2EPreset()` set?

## Decision Drivers

* The specifiers are resolved as Nx resolves them, from the workspace root, and only to modules that exist
* A package name that is no local module is still a package, so an unlisted plugin package is still reported
* The scopes of [ADR 0109](0109-nx-projects-as-scopes-of-the-package-that-owns-them.md) still run only the plugins a
  project calls for
* No new configuration

## Considered Options

* Resolve local specifiers in the `nx` plugin and gate config-less conventions on the scope's executors
* Hand the specifiers to the module resolver as imports
* Run every enabled plugin in every scope

## Decision Outcome

Chosen option: "Resolve local specifiers in the `nx` plugin and gate config-less conventions on the scope's
executors", because Nx's own lookup is small and only the `nx` plugin needs it, while the scopes keep running only the
plugins a project's files or targets call for.

### Local plugins, executors and generators

* A `plugins` entry of `nx.json` (a string or a `plugin` key), the executor of a target (`targets` of `nx.json`,
  `project.json` and the `nx` key of `package.json`), a `targetDefaults` key and a `generators` key name a local module
  when the part before `:` is a path relative to the workspace root, or matches a `paths` pattern (exact, or with one
  `*`) of the root `tsconfig.base.json`, else `tsconfig.json`, whose first target exists as a file, a file with a
  JavaScript or TypeScript extension or a directory. The target is relative to `baseUrl`, by default the tsconfig's
  directory. `extends` is not followed. Any other specifier is a package, as before.
* A local plugin in `plugins` is an entry: the file, the file with an extension, the `main` of a directory's
  `package.json` or its `index` file.
* For a local plugin, executor or generator, the plugin's `package.json` is the one in the target directory, else the
  closest one above the target below the workspace root. Each collection its `executors`, `builders`, `generators` and
  `schematics` keys name is read, and the `implementation`, `batchImplementation` and `factory` of each member of the
  collection's `executors`, `builders`, `generators` and `schematics` objects are entries, resolved as above relative
  to the collection file, without a `#export` suffix. An executor reads only the collections, as Nx does not load the
  plugin's module to run one. Every member of a collection is an entry, not only the named one, as a collection is the
  plugin's public interface.
* A workspace package for which the `nx` plugin is enabled reads the collections its own `package.json` names, as a
  local plugin that is a workspace member is a package of its own.
* A path outside the package that reads the config is that package's to analyse and is not an entry here; as in
  [ADR 0109](0109-nx-projects-as-scopes-of-the-package-that-owns-them.md), a local plugin with a `package.json` that is
  no workspace member stays outside the analysis.

### Config-less conventions in a scope

A plugin with no default globs and no config file in a scope also runs there when a target of the scope's
`project.json` runs an executor of the Nx plugin of the same name (`@nx/<id>:*` or `@nrwl/<id>:*`, `vite` for
`@nx/vite:build`), or of a known other name (`@nx/expo:*` for `expo-router`). The executors are read once per scope.
The plugin then applies what it knows without a config (`Plugin::resolve_without_config`, Vite's `index.html`) and its
workspace conventions. A project whose executors are another tool's still does not run the plugin.

### `nxE2EPreset()`

A Cypress `e2e` section that spreads an unknown value, in a config that imports from
`@nx/cypress/plugins/cypress-preset` (or `@nrwl/…`) and calls `nxE2EPreset`, has the preset's `supportFile`
(`<cypressDir>/support/e2e.{js,ts}`) and `specPattern` (`<cypressDir>/**/*.cy.{js,jsx,ts,tsx}`). `cypressDir` is the
string the source gives the option, else `src`. A key written after the last spread overrides the preset's value, one
written before it does not. As for `getJestProjects()` in
[ADR 0109](0109-nx-projects-as-scopes-of-the-package-that-owns-them.md), the call is recognised in the source.

### Consequences

* Good, because a local plugin is no longer an unlisted dependency, and its module and its executors' and generators'
  implementations are analysed: what they import is used, and what they leave unreached is still reported.
* Good, because an Nx project that builds with a tool's executor and that tool's defaults gets the tool's conventions.
* Good, because the support files of an Nx e2e project are entries.
* Bad, because only the root tsconfig's own `paths` are read, not those it inherits through `extends`.
* Bad, because a scope reads its `project.json` once more to find its executors.
* Neutral, because the gate is the executor's Nx plugin name; an executor whose package has another name runs no
  plugin's config-less conventions unless it is listed.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs` (`nx_local_plugins_executors_and_generators_are_followed`,
  `nx_executors_name_collections_not_modules`, `nx_collections_of_the_package_itself_are_entries`): relative and
  aliased plugins and executors, a wildcard alias, an executor no alias matches that stays a package, collection
  members with an extensionless path, an `index` file and a `#export` suffix, and a package's own collections.
* `crates/skif-js/tests/nx_integrated.rs` (`a_local_nx_plugin_named_by_an_alias_is_followed`): an aliased plugin is
  not unlisted and its module is not unused.
* `crates/skif-js-plugins/tests/nx_projects.rs`
  (`a_scope_whose_targets_run_a_tools_executor_applies_its_conventions`): `@nx/vite:build` without a Vite config makes
  the `index.html` script an entry, and a project with another executor does not.
* `crates/skif-js-plugins/tests/plugins.rs` (`cypress_nx_e2e_preset`): the defaults, `cypressDir`, a key after and
  before the spread, and another spread.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Resolve local specifiers in the `nx` plugin and gate config-less conventions on the scope's executors

* Good, because the lookup follows Nx's: the workspace root, the root tsconfig's `paths` and the plugin's
  `package.json`.
* Good, because scopes stay cheap: a plugin that a project neither configures nor runs is not run.
* Bad, because the `nx` plugin reads the root tsconfig itself, apart from the `typescript` plugin.

### Hand the specifiers to the module resolver as imports

* Good, because the resolver already knows the `paths` of every tsconfig.
* Bad, because a plugin input that is a dependency today would become an import of the config, judged by the module
  graph, and the collections, which are JSON files, would still need reading.

### Run every enabled plugin in every scope

* Good, because every config-less convention applies everywhere.
* Bad, because every plugin's conventions would apply to every project, also those of tools the project does not use
  (Vite's `index.html` in a webpack project), and each scope would run all plugins.

## More Information

* Amends: [ADR 0109](0109-nx-projects-as-scopes-of-the-package-that-owns-them.md) (a plugin with neither a config file
  nor default globs in a scope is not run there)
* Nx local plugins: <https://nx.dev/extending-nx/recipes/local-executors>
* Nx project configuration: <https://nx.dev/reference/project-configuration>
