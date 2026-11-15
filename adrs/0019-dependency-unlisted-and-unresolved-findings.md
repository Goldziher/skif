---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Dependency, Unlisted and Unresolved Findings

## Context and Problem Statement

The dependency rules (`unused-dependencies`, `unused-dev-dependencies`, `unlisted-dependencies`, `unresolved-imports`,
`unused-optional-peer-dependencies` and their relatives) decide whether an import, a config reference or a mock names a
package a manifest must list, which manifest that is, and whether a specifier that resolves to nothing is a defect. The
plugin model ([ADR 0015](0015-plugin-model-and-plugin-set.md)) says what each tool reads, and the analysis core
([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)) says in which order a specifier is resolved.
This record says how the findings are judged where the plugins, the resolver and the manifests meet: test-runner
mocks, builtin names, aliases and `tsconfig` mappings, generated paths, packages tool configs name, configs installed
below `node_modules`, resolve calls from another location, development entries, optional peers, ambient type
declarations and the TypeScript version that decides which `@types` packages are ambient.

## Decision Drivers

* A finding is reported where it is written and against the manifest that must list the package
* A finding follows how the runtime, the package manager, the bundler or the compiler resolves the construct, not a
  corpus exception
* References whose need skif cannot know (commands, plugin defaults, optional settings, environment-gated branches)
  produce no unlisted finding
* A finding that fails a test run fails a lint run by default, unless configuration says otherwise
* Rule ids and configuration keep covering the findings they cover, with details for consumers that want them apart

## Considered Options

* Judge each construct where it decides the need, by what the tool that resolves it does, with its own rule or detail
  where the advice differs
* Report every unresolved or unlisted name under the existing rules with one advice, and leave the rest to
  `ignore-dependencies` and `ignore-unresolved`
* Follow knip's resolution in every case, including merged `paths` and per-run plugin visitors

## Decision Outcome

Chosen option: "Judge each construct where it decides the need, by what the tool that resolves it does, with its own
rule or detail where the advice differs", because each case has a rule that holds beyond one repository, none needs
configuration, and a finding gets advice that fixes it in the package that must change.

### Test-runner mocks

* `stale-mocks` (`js`, default `error`) reports a mock call (`vi.*`, `jest.*`, `rs.*`/`rstest.*`, including
  `vi.mock(import('./x'))`, `requireActual` and `importActual`) whose relative, aliased or `#` specifier resolves to
  nothing. `unresolved-imports` does not report mocks.
* The specifier is resolved as an import is, with the same exemptions; `{ virtual: true }` mocks, fixture directories
  and ignored files are skipped.
* A bare specifier in a mock uses a listed package and never makes one unlisted.
* `ignore-unresolved` and `ignore-dependencies` silence both rules, and `stale-js-config` judges those lists whether
  or not `stale-mocks` runs.

### Imports

* A bare builtin name (`events`, `buffer`) uses a listed package of that name, since bundlers resolve it to the
  installed polyfill; a `node:` specifier uses no package. skif and knip differ here by design.
* An import that resolves through `paths` or another alias to a module of the analysis, and is no import of a package,
  uses no listed dependency, whatever name the specifier starts with; the listed dependency is judged on its other
  uses. An aliased import that reaches no file still uses the listed package whose name it starts with.
* A specifier naming a workspace package is an import of that package when an alias or `tsconfig` mapping leads into
  its directory, also into generated code that does not exist yet.
* An import that only another package's tsconfig `paths` map (no manifest lists it, nothing installs it, and the
  importing file's own tsconfig does not map it) is `unlisted-dependencies` with the detail `ForeignTsconfigPaths`,
  advice to add the mapping or import by package name, and a related location at the declaring config. The importing
  file's own tsconfig is the only one used for resolution.
* `declare module 'pkg'` covers every import of the package; `'pkg/*'` covers what it matches; `'pkg/x'` covers only
  that module.
* A type-only import or re-export (`import type`, `export type { T } from 'pkg'`, `export type * from 'pkg'`) is
  satisfied by `pkg` or by its `@types` package, since it is erased from the output and TypeScript resolves its types
  from either; the `@types` package it finds is used. An unlisted one advises adding the package or its types package.
* A package named only by a module imported from another workspace package is charged to that module's owner.

### Generated paths

* A relative specifier, a `tsconfig` `paths` target or a bundler alias target that leads into a missing path a
  `.gitignore` lists (of the root, of a package or of a directory between the file and the repository root) names a
  generated file: it is neither `unresolved-imports` nor `unlisted-dependencies`, whatever its name. This applies only
  after a `paths` pattern whose targets all miss has fallen through to the package and member lookup of
  [ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md) and found nothing.
* A specifier whose missing target is not ignored stays a finding: unresolved when it cannot name a package, a
  dependency of the package whose name it starts with otherwise.
* SvelteKit `./$types` is not unresolved while the extended generated tsconfig, or the types directory its `rootDirs`
  name, is absent and gitignored.

### Packages named by configs

* A package a config names itself as a required input (`DepRef::named_by_config`, including names a plugin derives
  from a value, and `PluginInput::FileDependency` for non-config files such as a Prisma schema) is
  `unlisted-dependencies` at that config when no manifest lists it, once per package, file and name, with the detail
  `ConfigReference`.
* Packages named by commands (judged with scripts, [ADR 0017](0017-scripts-binaries-and-working-directories.md)),
  plugin defaults and `PluginInput::OptionalDependency` do not qualify. A package a plugin requires on only one of the
  environment-gated variants of a config is such an optional reference
  ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)): it credits a listing and is never unlisted.
* A qualifying reference belongs to the innermost workspace package holding the config. The import exemptions apply:
  a runtime package, the package's own name, a peer of a listed package, an `engines` runtime, and (outside workspace
  isolation) a workspace member of a private package.
* A package a bundler alias `replacement` names (Vite, Vitest, webpack and Next.js `resolve.alias`) is judged against
  the package whose config is read, also when a module of another package that the config imports values from builds
  the alias, because the bundler resolves the replacement from that package's files. This is the one exception to
  charging a package named by an imported module to that module's owner. `skif-js-plugins` carries it as
  `PluginInput::ImporterDependency`.
* tsconfig `types`, `jsxImportSource`, `plugins` and `importHelpers` of a shared base config are satisfied by a listing
  of the base's owner or of the extending package. The compiler options of a rig profile are credits of the rig and of
  the projects that extend the profile ([ADR 0015](0015-plugin-model-and-plugin-set.md)).
* Configs that are gitignored, in fixture directories or matched by `ignore` are not judged. Production mode judges no
  config, so `--strict` judges none either.

### Installed configs

* A config below a `node_modules` directory is never judged itself, so nothing is reported there.
* The compiler options (`types`, `jsxImportSource`, `plugins`, `importHelpers`) of a `tsconfig` a package extends
  through `node_modules`, directly or through another installed config, apply to the extending package, as TypeScript
  merges the chain: the package an option names is a use of the extending package's listing.
* A `types` entry of such a config names nothing for the extending project when the first of the config's own
  `typeRoots` that holds those types is written relative to the config and lies in its install, since TypeScript loads
  them from there. A root written with `${configDir}` is the project's own, and a config without `typeRoots`, or whose
  roots hold nothing, keeps the rule above.

### Packages another package resolves

* The `resolve` of a `createRequire(location)` call names a module of the file only when `location` is
  `import.meta.url`, `import.meta.filename` or `__filename`. Any other location (a path built with `join`, a variable,
  `__dirname`) records no import: the specifier is neither a dependency of the file's package nor a file it keeps in
  use, because the package that resolves it owns the dependency.
* A `require` bound to `createRequire(…)` and the other resolve calls are imports of the file
  ([ADR 0018](0018-export-namespace-and-member-rules.md)).

### Development entries and optional peers

* `unused-dev-dependencies` reports a `devDependencies` entry of a package that the same manifest lists in
  `dependencies`, used or not, with the advice to remove it from `devDependencies` and an unsafe fix that removes it.
  A listing in `optionalDependencies` or `peerDependencies` beside it is no duplicate. A package that is not judged and
  an `ignore-dependencies` entry leave it unreported.
* `unused-optional-peer-dependencies` counts an optional peer as installed when the package, a package whose directory
  contains it or the root lists it in `dependencies`, `devDependencies` or `optionalDependencies`. With
  `isolate-workspaces`, and so with `strict`, only the package's own manifest counts.
* Only the configured `ignore-dependencies` lists silence that rule; a plugin reference that may be absent (a package a
  bundler keeps external) does not.

### Install hooks

* `.pnpmfile.cjs` and `.pnpmfile.mjs` are entries but not source: a package whose only files are install hooks has
  unknown entries and is listed among the incomplete packages ([ADR 0022](0022-reporting-and-output-formats.md)).

### Workspace isolation

* By default a containing package's or the root's listing satisfies a member's use. With `[js] isolate-workspaces` (or
  `--isolate-workspaces`) each workspace package uses only its own manifest, for imports, script binaries and config
  references alike, without production mode: tool configs are still judged and any dependency field satisfies an
  import. `[js] strict` (or `--strict`) adds production mode and knip's direct-dependency rules
  ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)).

### Ambient `@types` under TypeScript 6

* A package's TypeScript major comes from the `typescript` installed nearest to it, else the lowest version its own
  declared ranges admit (dependency fields before `peerDependencies`, catalogs resolved), else the root's.
* The effective `types` and `typeRoots` of its tsconfig decide, each replacing what an extended config sets: with no
  `types`, TypeScript 6 and later include no `@types` package, and an earlier or unknown version includes all;
  `types: ["*"]` always includes all; `typeRoots` without `node_modules/@types` include none. A solution config is
  judged by the widest of the configs it references.

### Consequences

* Good, because a project can baseline or turn off stale mocks without hiding unresolved imports, and a stale mock
  still fails a run by default.
* Good, because a tsconfig that extends a package, a lint config that loads a plugin and a schema that names a
  generator nobody installed are reported where they are written.
* Good, because a dependency that a mapping only shadows is reported unused, and a `paths` entry into generated
  typings needs no ignore entry.
* Good, because a shared alias factory needs no dependency on what its consumers bundle, a package extending a rig
  profile uses the `tslib` it lists for the profile's `importHelpers`, and types a rig brings in its own install are
  not asked of the project.
* Good, because a test that resolves a file through another package's install is not told to list that package's
  dependencies, an optional peer the root installs is not reported in each member, and a redundant development entry
  is reported with a fix.
* Good, because under TypeScript 6 a globals-only `@types` package nothing names is reported unused, and a project on an
  earlier TypeScript keeps its report.
* Bad, because a config that names a package no manifest lists fails the run with `unlisted-dependencies`.
* Bad, because a project that relies on knip's merged `paths` gets errors knip does not report.
* Bad, because an import into a gitignored path that is never generated is not reported, and a package extending an
  installed config without listing what its compiler options need is not reported at the installed config.
* Bad, because a polyfill listed but used only by Node.js code is not reported unused, and an optional peer the root
  installs passes for a member published on its own.
* Bad, because `createRequire(pathToFileURL(__filename))` counts as another location, so the package its `resolve`
  names is not credited.
* Bad, because a lint without an install reads declared TypeScript ranges, not the locked version, and cannot see what
  an installed config's type roots hold.

### Confirmation

* `crates/skif-js/tests/fixtures.rs`: `stale_mocks_snapshot`,
  `a_mock_of_a_missing_module_is_a_stale_mock_not_an_unresolved_import`, `ignore_unresolved_silences_a_stale_mock`,
  `stale_mocks_can_be_turned_off_without_bringing_the_mocks_back_as_unresolved_imports` and
  `stale_js_config_judges_the_ignore_lists_with_stale_mocks_off`.
* `crates/skif-js/tests/foreign_tsconfig_paths.rs` and `crates/skif-js/tests/paths_before_packages.rs`;
  `crates/skif-js/tests/fp_graph.rs`: `a_listed_package_a_path_alias_maps_into_own_sources_is_unused`.
* `crates/skif-js/tests/config_references.rs`: `packages_a_tsconfig_names_are_unlisted_at_the_tsconfig`,
  `listed_member_command_and_optional_references_are_not_unlisted`,
  `ignores_fixtures_and_production_mode_apply_to_config_references`,
  `compiler_options_of_a_shared_base_config_are_satisfied_by_the_extending_package`,
  `packages_an_imported_shared_config_names_are_charged_to_its_package`,
  `a_package_named_only_on_an_environment_gated_branch_is_optional`,
  `a_package_named_on_a_branch_of_another_unknown_condition_stays_required`,
  `an_alias_replacement_from_a_shared_config_factory_is_judged_against_the_reader`,
  `compiler_options_of_an_installed_base_config_are_used_by_the_extending_package`,
  `types_an_installed_config_finds_in_its_own_type_roots_come_with_its_package` and
  `an_optional_peer_a_bundler_keeps_external_is_reported_unless_the_configuration_ignores_it`.
* `crates/skif-js-plugins/tests/plugins.rs`: `packages_a_config_names_are_recorded_at_the_file_that_names_them`,
  `packages_named_only_to_tune_a_tool_are_optional_references` and
  `typescript_types_an_installed_config_finds_in_its_own_type_roots_name_no_package`.
* `crates/skif-js/tests/dependency_credits.rs`: `a_type_only_reexport_is_satisfied_by_the_types_package` and
  `a_dev_dependency_the_manifest_also_lists_in_dependencies_is_reported`.
* `crates/skif-js/tests/installed.rs`: `an_optional_peer_the_root_installs_is_installed_for_the_member`.
* `crates/skif-js/tests/fp_resolve.rs`: `specifiers_into_missing_gitignored_paths_are_generated_files`,
  `types_packages_stay_ambient_without_a_types_option_before_typescript_6`,
  `types_packages_are_not_ambient_without_a_types_option_on_an_installed_typescript_6`,
  `a_types_wildcard_keeps_every_types_package_ambient_on_typescript_6`,
  `a_typescript_range_is_read_by_the_lowest_version_it_admits` and
  `a_solution_tsconfig_is_judged_by_the_configs_it_references`.
* `crates/skif-js-syntax/tests/module_info.rs`: `a_create_require_resolve_names_a_module_only_from_this_file`.
* `crates/skif-js/tests/unlisted_parity.rs`: `a_declared_subpath_declares_only_that_module`,
  `a_workspace_package_through_an_alias_into_generated_code_is_that_package` and
  `a_create_require_of_another_package_resolves_that_packages_dependencies`.
* `crates/skif-js/tests/monorepo_regressions.rs`:
  `a_pnpmfile_alone_does_not_give_a_project_source_whose_dependencies_are_judged` and
  `bare_builtin_imports_use_a_listed_package_of_that_name`.
* `crates/skif-js/src/resolve.rs`: `sveltekit_route_types_are_unknown_while_the_extended_config_is_not_generated` and
  `sveltekit_route_types_follow_the_root_dirs_of_the_generated_config`.
* `crates/skif-js/tests/isolate_workspaces.rs`; `crates/skif-cli/tests/filters.rs`:
  `isolate_workspaces_reports_a_members_use_of_an_ancestor_only_dev_dependency` and
  `isolate_workspaces_is_not_strict_mode`.

## Pros and Cons of the Options

### Judge each construct where it decides the need, by what the tool that resolves it does, with its own rule or detail where the advice differs

* Good, because findings land at the file a user edits, in the manifest that must change, with advice that fits.
* Bad, because one more rule id and several details are public and have to stay, and the rule that charges a package
  named by an imported module to its owner has an exception.

### Report every unresolved or unlisted name under the existing rules with one advice, and leave the rest to `ignore-dependencies` and `ignore-unresolved`

* Good, because the rule set stays smaller.
* Bad, because mocks cannot be triaged apart from imports, the advice for a foreign `paths` mapping or a config
  reference is wrong, and a shared factory's package is told to list what it never bundles.

### Follow knip's resolution in every case, including merged `paths` and per-run plugin visitors

* Good, because the findings would match knip more often.
* Bad, because a merged `paths` map resolves imports the package's own build cannot, hiding real failures, and knip
  reports a generated import as unlisted where the build contradicts it.
