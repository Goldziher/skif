---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Generated Paths, Alias Replacements, Type-Only Re-Exports and Compiler Options of Installed Configs

## Context and Problem Statement

[ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md) decides where `unlisted-dependencies` and
`unresolved-imports` judge an import or a config reference. Five cases on the pinned corpus fall outside its wording or
contradict it:

* A specifier that leads into a missing path a `.gitignore` lists: a `tsconfig` `paths` entry named like a package
  (`"electron": ["../.build/typings/electron.d.ts"]`), a bundler alias or a relative path. ADR 0019 says such a
  specifier is no finding only for an alias into a workspace package and for SvelteKit `./$types`.
* A bundler alias `replacement` naming a package (`{ find: 'stream', replacement: 'stream-browserify' }`) that a shared
  config factory of another workspace package builds. ADR 0019 charges a package named only by a module imported from
  another workspace package to that module's owner, so the finding lands on the factory's package, which bundles
  nothing.
* A type-only re-export (`export type { JSONSchema7 } from 'json-schema'`) was not satisfied by the listed `@types`
  package, while a type-only import of the same package was. The advice named a package that ships no types.
* A compiler option of a `tsconfig` that a package extends through its own `node_modules` (a rig profile,
  `./node_modules/<rig>/profiles/app/tsconfig-base.json`). ADR 0019 judges no config below `node_modules`, so the
  `tslib` that the profile's `importHelpers` needs was never credited, and the package listing it saw it as unused.
* Jest's `modulePathIgnorePatterns`: whether the files it matches are left out of the analysis.

## Decision Drivers

* A finding follows how the compiler, the bundler or the runtime resolves the construct (ADR 0019)
* A finding is reported against the manifest that must list the package, where the construct is written
* An accepted record is not edited; a changed decision is a new record
* The knip comparison stays explainable: a difference is a skif fix, a named deviation or a knip false finding

## Considered Options

* Decide each case by what the tool that resolves it does
* Keep ADR 0019 as written and leave the cases to `ignore-dependencies` and `ignore-unresolved`
* Follow knip in each case

## Decision Outcome

Chosen option: "Decide each case by what the tool that resolves it does", because each rule holds for every
repository with the construct, and none needs configuration.

### Specifiers into missing ignored paths

* A relative specifier, a `tsconfig` `paths` target or a bundler alias target that leads into a missing path a
  `.gitignore` lists (of the root, of a package or of a directory between the file and the repository root) names a
  generated file: neither `unresolved-imports` nor `unlisted-dependencies`, whatever the specifier is named like. The
  build or the tool generates it before the code is compiled.
* A specifier whose missing target is not ignored stays a finding: unresolved when it cannot name a package, a
  dependency of the package it starts like otherwise, as before.

### Bundler alias replacements

* A package an alias `replacement` names (Vite, Vitest, webpack and Next.js `resolve.alias`) is judged against the
  package whose config is read, also when a module of another package that the config imports values from builds
  the alias. The bundler resolves the replacement from the files it bundles, which are that package's. This supersedes,
  for alias replacements only, the rule of ADR 0019 (*Imports*) that charges a package named by an imported module to
  that module's owner.
* `skif-js-plugins` carries such a package as `PluginInput::ImporterDependency`; an environment-gated one is optional
  as any other ([ADR 0036](0036-packages-named-on-an-environment-gated-branch.md)).

### Type-only re-exports

* A type-only re-export (`export type { T } from 'pkg'`, `export type * from 'pkg'`) is satisfied by `pkg` or by its
  `DefinitelyTyped` package, as a type-only import is: it is erased from the output, and TypeScript resolves its types
  from either. The `@types` package it finds is used.
* An unlisted type-only import or re-export advises adding the package or its types package.

### Compiler options of installed configs

* A compiler option (`types`, `jsxImportSource`, `plugins`, `importHelpers`) of a `tsconfig` that a package extends
  through a `node_modules` directory, directly or through another installed config, applies to the package: TypeScript
  merges the chain into its options. The package it names is a use of the extending package's manifest that lists it.
* The installed config is still not judged, so nothing is reported there when no manifest lists the package.

### Jest `modulePathIgnorePatterns`

* skif does not read `modulePathIgnorePatterns` to leave files out. It hides modules from Jest's module loader, not
  from the project: TypeScript, the linters and the build still read those files. knip's Jest plugin does not read it
  either.
* The haste test data that jest's own config hides this way lies in directories with a `package.json` of their own,
  which skif does not analyse as part of the enclosing package.

### Consequences

* Good, because a paths entry into generated typings, such as an `electron.d.ts` the build writes, no longer needs an
  ignore entry.
* Good, because a shared alias factory needs no dependency on what its consumers bundle, and a consumer that lists
  the replacement uses it.
* Good, because a package that re-exports types from a package typed by `DefinitelyTyped` needs only the `@types`
  package, and the advice names it.
* Good, because a package that extends a rig's profile uses the `tslib` it lists for the profile's `importHelpers`.
* Bad, because an import into a git-ignored path that is never generated is not reported.
* Bad, because a package extending an installed config without listing what its compiler options need is not
  reported.
* Bad, because a Jest test hidden by `modulePathIgnorePatterns` in the package's own tree is still analysed as a test
  file.

### Confirmation

* `crates/skif-js/tests/fp_resolve.rs`: `specifiers_into_missing_gitignored_paths_are_generated_files`.
* `crates/skif-js/tests/config_references.rs`:
  `an_alias_replacement_from_a_shared_config_factory_is_judged_against_the_reader` and
  `compiler_options_of_an_installed_base_config_are_used_by_the_extending_package`.
* `crates/skif-js/tests/dependency_credits.rs`: `a_type_only_reexport_is_satisfied_by_the_types_package`.

## Pros and Cons of the Options

### Decide each case by what the tool that resolves it does

* Good, because each finding gives advice that fixes it, in the package that must change.
* Bad, because ADR 0019's rule for imported modules gains an exception.

### Keep ADR 0019 as written and leave the cases to `ignore-dependencies` and `ignore-unresolved`

* Good, because nothing changes.
* Bad, because the factory's package is told to list what it never bundles, and a listed `tslib` is reported unused.

### Follow knip in each case

* Good, because the comparison has fewer differences.
* Bad, because knip reports the generated `electron` import as unlisted, which the build contradicts, and leaves a
  listed `tslib` of a package extending a rig profile uncredited.
