---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Dependency, Unlisted and Unresolved Findings

## Context and Problem Statement

The dependency rules (`unused-dependencies`, `unlisted-dependencies`, `unresolved-imports` and their relatives) decide
whether an import, a config reference or a mock names a package a manifest must list, which manifest that is, and
whether a specifier that resolves to nothing is a defect. The plugin model
([ADR 0015](0015-plugin-model-and-plugin-set.md)) says what each tool reads; this record says how the findings are
judged where the plugins, the resolver and the manifests meet: test-runner mocks, builtin names, `tsconfig` mappings of
other packages, packages tool configs name, ambient type declarations and the TypeScript version that decides which
`@types` packages are ambient.

## Decision Drivers

* A finding is reported where it is written and against the manifest that must list the package
* A finding follows how the runtime, the bundler or the compiler resolves the construct, not a corpus exception
* References whose need skif cannot know (commands, plugin defaults, optional settings) produce no finding
* A finding that fails a test run fails a lint run by default, unless configuration says otherwise
* Rule ids and configuration keep covering the findings they covered, with details for consumers that want them apart

## Considered Options

* Judge each construct where it decides the need, with its own rule or detail where the advice differs
* Report every unresolved or unlisted name under the existing rules with one advice
* Follow knip's resolution in every case, including merged `paths` and per-run plugin visitors

## Decision Outcome

Chosen option: "Judge each construct where it decides the need, with its own rule or detail where the advice differs",
because each case has a rule that holds beyond one repository, and a finding gets advice that fixes it.

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
* An import that only another package's tsconfig `paths` map (no manifest lists it, nothing installs it, and the
  importing file's own tsconfig does not map it) is `unlisted-dependencies` with the detail `ForeignTsconfigPaths`,
  advice to add the mapping or import by package name, and a related location at the declaring config. The importing
  file's own tsconfig is the only one used for resolution.
* A specifier naming a workspace package is an import of that package when an alias or `tsconfig` mapping leads into
  its directory, also into generated code that does not exist yet.
* `declare module 'pkg'` covers every import of the package; `'pkg/*'` covers what it matches; `'pkg/x'` covers only
  that module.
* SvelteKit `./$types` is not unresolved while the extended generated tsconfig, or the types directory its `rootDirs`
  name, is absent and gitignored.
* A package named only by a module imported from another workspace package is charged to that module's owner.

### Packages named by configs

* A package a config names itself as a required input (`DepRef::named_by_config`, including names a plugin derives
  from a value, and `PluginInput::FileDependency` for non-config files such as a Prisma schema) is
  `unlisted-dependencies` at that config when no manifest lists it, once per package, file and name, with the detail
  `ConfigReference`.
* Packages named by commands (judged with scripts, [ADR 0017](0017-scripts-binaries-and-working-directories.md)),
  plugin defaults and `PluginInput::OptionalDependency` do not qualify.
* A qualifying reference belongs to the innermost workspace package holding the config. The import exemptions apply:
  a runtime package, the package's own name, a peer of a listed package, an `engines` runtime, and (outside workspace
  isolation) a workspace member of a private package.
* tsconfig `types`, `jsxImportSource`, `plugins` and `importHelpers` of a shared base config are satisfied by a listing
  of the base's owner or of the extending package.
* Configs below `node_modules`, gitignored, in fixture directories or matched by `ignore` are not judged. Production
  mode judges no config, so `--strict` judges none either.

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
* Good, because the advice of a foreign-`paths` finding names the fix.
* Good, because under TypeScript 6 a globals-only `@types` package nothing names is reported unused, and a project on an
  earlier TypeScript keeps its report.
* Bad, because a config that names a package no manifest lists fails the run with `unlisted-dependencies`.
* Bad, because a project that relies on knip's merged `paths` gets errors knip does not report.
* Bad, because a polyfill listed but used only by Node.js code is not reported unused.
* Bad, because a lint without an install reads declared TypeScript ranges, not the locked version.

### Confirmation

* `crates/skif-js/tests/fixtures.rs`: `stale_mocks_snapshot`,
  `a_mock_of_a_missing_module_is_a_stale_mock_not_an_unresolved_import`, `ignore_unresolved_silences_a_stale_mock`,
  `stale_mocks_can_be_turned_off_without_bringing_the_mocks_back_as_unresolved_imports` and
  `stale_js_config_judges_the_ignore_lists_with_stale_mocks_off`.
* `crates/skif-js/tests/foreign_tsconfig_paths.rs`.
* `crates/skif-js/tests/config_references.rs`: `packages_a_tsconfig_names_are_unlisted_at_the_tsconfig`,
  `listed_member_command_and_optional_references_are_not_unlisted`,
  `ignores_fixtures_and_production_mode_apply_to_config_references`,
  `compiler_options_of_a_shared_base_config_are_satisfied_by_the_extending_package` and
  `packages_an_imported_shared_config_names_are_charged_to_its_package`.
* `crates/skif-js-plugins/tests/plugins.rs`: `packages_a_config_names_are_recorded_at_the_file_that_names_them` and
  `packages_named_only_to_tune_a_tool_are_optional_references`.
* `crates/skif-js/tests/unlisted_parity.rs`: `a_declared_subpath_declares_only_that_module` and
  `a_workspace_package_through_an_alias_into_generated_code_is_that_package`.
* `crates/skif-js/tests/monorepo_regressions.rs`:
  `a_pnpmfile_alone_does_not_give_a_project_source_whose_dependencies_are_judged` and
  `bare_builtin_imports_use_a_listed_package_of_that_name`.
* `crates/skif-js/src/resolve.rs`: `sveltekit_route_types_are_unknown_while_the_extended_config_is_not_generated` and
  `sveltekit_route_types_follow_the_root_dirs_of_the_generated_config`.
* `crates/skif-js/tests/fp_resolve.rs`: `types_packages_stay_ambient_without_a_types_option_before_typescript_6`,
  `types_packages_are_not_ambient_without_a_types_option_on_an_installed_typescript_6`,
  `a_types_wildcard_keeps_every_types_package_ambient_on_typescript_6`,
  `a_typescript_range_is_read_by_the_lowest_version_it_admits` and
  `a_solution_tsconfig_is_judged_by_the_configs_it_references`.
* `crates/skif-js/tests/isolate_workspaces.rs`; `crates/skif-cli/tests/filters.rs`:
  `isolate_workspaces_reports_a_members_use_of_an_ancestor_only_dev_dependency` and
  `isolate_workspaces_is_not_strict_mode`.

## Pros and Cons of the Options

### Judge each construct where it decides the need, with its own rule or detail where the advice differs

* Good, because findings land at the file a user edits, with advice that fits.
* Bad, because one more rule id and several details are public and have to stay.

### Report every unresolved or unlisted name under the existing rules with one advice

* Good, because the rule set stays smaller.
* Bad, because mocks cannot be triaged apart from imports, and the advice for a foreign `paths` mapping or a config
  reference is wrong.

### Follow knip's resolution in every case, including merged `paths` and per-run plugin visitors

* Good, because the findings would match knip more often.
* Bad, because a merged `paths` map resolves imports the package's own build cannot, hiding real failures.
