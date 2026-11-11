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
* Treat an absent `types` as `[]` on every version

## Decision Outcome

Chosen option: "Read the TypeScript major version of each package, from its install, else its declared range, else
the root's", because the installed compiler is the one that runs, and the declared ranges cover a lint without an
install.

The major version of a package is that of the `typescript` installed in the nearest `node_modules` from its directory
up to the workspace root (the `version` of `node_modules/typescript/package.json`); else the lowest version the
`typescript` range of its `dependencies`, `devDependencies` or `optionalDependencies` admits, its `peerDependencies`
range when it lists none of those; else the same for the root manifest. A range names its lowest version as the
specifier model of [ADR 0006](0006-specifier-and-semver-model.md) reads it: `^6.0.0`, `>=6`, `~6.0.2` and `6` are 6. A
range without a version (`*`, `latest`, a tag, a `catalog:` reference) and no install leave the version unknown.

The `tsconfig.json` chain then decides as before, with one more outcome: a `types` list without `*` along the chain, a
missing config or `typeRoots` without `node_modules/@types` include no package; `types: ["*"]` includes every package
on every version; and a chain without `types` includes every package below TypeScript 6 or when the version is
unknown, and none from TypeScript 6.

### Consequences

* Good, because under TypeScript 6 a globals-only `@types` package that nothing names is reported as unused.
* Good, because a repository that has not moved to TypeScript 6, or whose version cannot be told, keeps its report.
* Neutral, because a package with a TypeScript 6 install and a `^5` range is judged by TypeScript 6, the compiler that
  runs.
* Bad, because a package that compiles with a TypeScript from outside the workspace, or only with
  `@typescript/native-preview`, is judged by the version its own ranges name, or keeps the earlier default.

### Confirmation

* `crates/skif-js/tests/fp_resolve.rs`: `types_packages_stay_ambient_without_a_types_option_before_typescript_6`,
  `types_packages_are_not_ambient_without_a_types_option_on_an_installed_typescript_6`,
  `types_packages_are_not_ambient_without_a_types_option_on_a_declared_typescript_6`,
  `a_types_wildcard_keeps_every_types_package_ambient_on_typescript_6` and
  `a_member_without_typescript_follows_the_typescript_of_the_workspace_root`.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read the TypeScript major version of each package, from its install, else its declared range, else the root's

* Good, because the installed compiler decides when there is one, and a lint without an install still gets a version.
* Bad, because each package whose chain sets no `types` reads up to one manifest per directory up to the root.

### Read only the declared `typescript` range

* Good, because no file outside the manifests is read.
* Bad, because a wide range (`>=5`) or a range without a version decides against the compiler that is installed.

### Treat an absent `types` as `[]` on every version

* Good, because it is the simplest rule and matches TypeScript from version 6.
* Bad, because every repository still on TypeScript 5 gets false unused findings for packages its compiler includes.

## More Information

* TypeScript `types` option: <https://www.typescriptlang.org/tsconfig/#types>
* Issue [#201](https://github.com/Goldziher/skif/issues/201)
