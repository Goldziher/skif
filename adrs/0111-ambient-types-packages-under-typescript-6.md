---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
---

# Ambient `@types` Packages Under TypeScript 6

## Context and Problem Statement

A globals-only `@types` package (`@types/chrome`, `@types/mocha`, `@types/webpack-env`) has no module to import: a
`tsconfig.json` that includes every visible `@types/*` package is its only use. skif credited such a package to every
package whose `tsconfig.json` chain sets no `compilerOptions.types`, or sets `types: ["*"]`, and keeps
`node_modules/@types` among its `typeRoots`.

TypeScript 6 changed the default of `types` from "every package of the type roots" to `[]`: without `types`, no
`@types/*` package is included unless an import, a `types` entry or a `/// <reference types>` names it, and
`types: ["*"]` restores the earlier default. Under TypeScript 6, skif kept crediting a package that only the earlier
default had used, so it was never reported as unused.

How does skif tell which default a package compiles with?

## Decision Drivers

* An absent `types` means what the package's own TypeScript makes of it
* No false unused finding when the version cannot be told: the earlier behaviour is the safe side
* No new configuration, and library crates read files only through the `FileSystem` trait
* `types: ["*"]` keeps its meaning on every version

## Considered Options

* Read the TypeScript major version of each package, from its install, else its declared range, else the root's
* Read only the declared `typescript` range
* Read the `typescript` version the lockfile records
* Treat an absent `types` as `[]` on every version

## Decision Outcome

Chosen option: "Read the TypeScript major version of each package, from its install, else its declared range, else
the root's", because the installed compiler is the one that runs, and the declared ranges cover a lint without an
install.

The major version of a package is, in order:

1. That of the `typescript` installed in the nearest `node_modules` from its directory up to the workspace root: the
   `version` of `node_modules/typescript/package.json`, where a manifest that does not parse is passed over as for any
   other installed package, and a prerelease counts by its major (`6.0.0-beta` is 6).
2. Else the lowest version the `typescript` ranges of the package admit. The entries under `dependencies`,
   `devDependencies` or `optionalDependencies` decide when there are any, even when none of them names a version; its
   `peerDependencies` range, which describes its consumers, only when it has none of those. A `catalog:` or
   `catalog:<name>` reference is read from the workspace catalogs.
3. Else the same for the root manifest.

A range is read by the lowest version the npm range admits, as `nodejs-semver` computes it from the specifier model of
[ADR 0006](0006-specifier-and-semver-model.md): `^6.0.0`, `>=6`, `~6.0.2`, `6`, `>=6 <8` and `6.1.0-dev.20261101` are
6; `^5 || ^6` is 5; an upper bound alone (`<6`, `<6.0.0`, `<=5.9.3`) admits `0.0.0`, so it is below 6. A specifier
without a range (`*`, `latest`, a tag, a path, a `catalog:` reference the catalogs do not resolve) names no version,
and when nothing names one the version is unknown.

The `tsconfig.json` of the package then decides from its effective `types` and `typeRoots`. Each is the value the
config sets itself, else the one of the nearest config it extends, a later `extends` entry before an earlier one, as
TypeScript replaces an inherited compiler option rather than merging it: a child `types: ["*"]` over a parent
`types: []` includes every package, and a child `types: []` over a parent `types: ["*"]` none. A missing config,
`typeRoots` without `node_modules/@types` or a `types` list without `*` include no package; `types: ["*"]` includes
every package on every version; and no `types` along the chain includes every package below TypeScript 6 or when the
version is unknown, and none from TypeScript 6.

A solution config, one whose `references` name other configs (Vite's `files: []` beside `tsconfig.app.json` and
`tsconfig.node.json`), compiles through those configs. The referenced configs inside its directory are read too, and
the widest outcome of the solution config and those configs decides: every package when one of them includes every
package, else the version-dependent default when one of them sets no `types`.

### Consequences

* Good, because under TypeScript 6 a globals-only `@types` package that nothing names is reported as unused.
* Good, because a repository that has not moved to TypeScript 6, or whose version cannot be told, keeps its report.
* Neutral, because a package with a TypeScript 6 install and a `^5` range is judged by TypeScript 6, the compiler that
  runs.
* Neutral, because a solution config is judged by its widest referenced config: an `@types` package that only one
  referenced project includes counts as used for the whole package.
* Bad, because a package that compiles with a TypeScript from outside the workspace, or only with
  `@typescript/native-preview`, is judged by the version its own ranges name, or keeps the earlier default.
* Bad, because a lint without an install reads the declared range, not the locked version: `^5.4.0` locked at 6.0.2
  counts as 5.

### Confirmation

* `crates/skif-js/tests/fp_resolve.rs`: `types_packages_stay_ambient_without_a_types_option_before_typescript_6`,
  `types_packages_are_not_ambient_without_a_types_option_on_an_installed_typescript_6`,
  `types_packages_are_not_ambient_without_a_types_option_on_a_declared_typescript_6`,
  `a_types_wildcard_keeps_every_types_package_ambient_on_typescript_6`,
  `a_member_without_typescript_follows_the_typescript_of_the_workspace_root`,
  `the_types_option_of_a_config_replaces_the_one_it_extends`, `a_later_extends_entry_overrides_an_earlier_one`,
  `the_type_roots_of_a_config_replace_the_ones_it_extends`,
  `a_typescript_range_is_read_by_the_lowest_version_it_admits`,
  `a_catalog_typescript_range_is_read_from_the_workspace_catalogs`,
  `a_peer_typescript_range_counts_only_without_another_typescript_entry`,
  `a_solution_tsconfig_is_judged_by_the_configs_it_references`, `a_typescript_6_prerelease_counts_as_typescript_6`
  and `a_member_follows_the_nearest_typescript_install`.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read the TypeScript major version of each package, from its install, else its declared range, else the root's

* Good, because the installed compiler decides when there is one, and a lint without an install still gets a version.
* Bad, because a package whose chain sets no `types` reads up to one manifest per directory up to the root; the
  manifests are cached, so a shared install is read once.

### Read only the declared `typescript` range

* Good, because no file outside the manifests is read.
* Bad, because a wide range (`>=5`) or a range without a version decides against the compiler that is installed.

### Read the `typescript` version the lockfile records

Considered as a step between the install and the declared range, and left out.

* Good, because a lint without an install would get the resolved version rather than the lowest one a range admits.
* Bad, because the lockfile facts skif reads for packages without an install (their binaries and peers, from
  `pnpm-lock.yaml`, `package-lock.json`, `bun.lock` and Berry's `yarn.lock`) record no version, so each of the four
  parsers would have to learn to read one.
* Bad, because a lockfile may lock several `typescript` versions, and telling which one a package compiles with means
  reading each format's per-importer resolution rather than a package name.

### Treat an absent `types` as `[]` on every version

* Good, because it is the simplest rule and matches TypeScript from version 6.
* Bad, because every repository still on TypeScript 5 gets false unused findings for packages its compiler includes.

## More Information

* TypeScript `types` option: <https://www.typescriptlang.org/tsconfig/#types>
* Issue [#201](https://github.com/Goldziher/skif/issues/201)
