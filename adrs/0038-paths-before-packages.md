---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# `paths` Before Packages: TypeScript's Order for Mappings Named Like a Dependency

## Context and Problem Statement

A `paths` mapping can start with the name of a package the project lists: `src/*` to `./src/*` beside a workspace
member named `src`, `lib/*` to `./src/lib/*` beside an installed `lib`. TypeScript applies `paths` before it looks in
`node_modules` or at workspace packages, so `import 'src/Feature/X'` is the importer's own `src/Feature/X.ts`, and the
listed `src` is not used by it. skif followed knip in two places instead:

* an import that a `tsconfig.json` mapping led into the importer's own sources still counted as a use of the listed
  dependency whose name the specifier starts with, so `unused-dependencies` missed it;
* the `paths` of `[js.workspaces.<dir>.paths]` were tried only after installed packages and workspace members, so the
  import went to the package and the local file was reported by `unused-files`.

The resolution rules of [ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md) and the import rules of
[ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md) did not say which comes first. Which order does skif
follow, and when does an aliased import use a listed dependency?

## Decision Drivers

* The compiler and the bundler decide what an import reaches; a finding that disagrees with them is wrong for the user
  whatever knip reports
* `[js.workspaces.<dir>.paths]` stands in for a `tsconfig` skif cannot read, so it behaves as the `tsconfig` mapping
  would
* A dependency is used when the code reaches it, not when a specifier merely starts with its name
* A mapping whose targets are missing must not hide the package it is named after
* Every place skif deliberately departs from knip is counted by the comparison harness
  ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md))

## Considered Options

* Follow TypeScript: `paths` before packages and members, and a local target uses no listed dependency
* Keep knip's order and document it
* Follow TypeScript, but keep crediting the listed dependency of a mapping into local sources

## Decision Outcome

Chosen option: "Follow TypeScript: `paths` before packages and members, and a local target uses no listed
dependency", because it reports what the compiler resolves, and a dependency is used only when an import reaches it.

### Resolution

* The `paths` of `[js.workspaces.<dir>.paths]` are tried before lookup directories (`baseUrl`, `resolve.modules`),
  installed packages and workspace members, for scripts and stylesheets alike, innermost package first. The `paths` of
  the importer's `tsconfig.json` already come before installed packages inside the resolver.
* A pattern whose targets all miss, generated code that is not there yet included, falls through to the package or
  member lookup, as TypeScript does. Nothing found there leaves the specifier generated, unresolved or taken for the
  package it names, as before.
* A mapping that leads into a workspace member's directory, or into `node_modules`, is an import of that package as
  [ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md) states; one that leads into the importer's own
  sources is a local import.

### Dependency use

* An import that resolves through `paths` or another alias to a module of the analysis, and is no import of a package,
  uses no listed dependency, whatever name the specifier starts with. The listed dependency is then judged by
  `unused-dependencies` on its other uses.
* An aliased import that reaches no file (an unresolved target, generated code that is not there yet, a computed alias)
  still uses the listed package whose name it starts with, as does a bare builtin name.
* The departure from knip is the known deviation `paths-before-packages` of the comparison harness: knip credits the
  listed dependency and reports the local file of `[js.workspaces.<dir>.paths]` unused, skif reports the dependency.

### Consequences

* Good, because a dependency a mapping only shadows is reported by `unused-dependencies` and can be removed.
* Good, because a local file a `[js.workspaces.<dir>.paths]` mapping reaches is no longer reported by `unused-files`.
* Good, because `[js.workspaces.<dir>.paths]` and `tsconfig.json` `paths` resolve in the same order.
* Bad, because a project that lists a package only for consumers while a development alias points at a local copy of
  its sources now gets that dependency reported, and needs `ignore-dependencies` for it.
* Bad, because the counts against knip gain one-sided findings, set apart as `paths-before-packages`.

### Confirmation

* `crates/skif-js/tests/paths_before_packages.rs`: the member and installed-package variants for `tsconfig.json`
  `paths` and for `[js.workspaces.<dir>.paths]`, a mapping whose target is missing falling through to the member and
  to the installed package, and an import no `paths` entry matches still using the listed package.
* `crates/skif-js/tests/fp_graph.rs`: `a_listed_package_a_path_alias_maps_into_own_sources_is_unused`.

## Pros and Cons of the Options

### Follow TypeScript: `paths` before packages and members, and a local target uses no listed dependency

* Good, because findings match what the compiler and the bundler resolve.
* Good, because a dependency counts as used only when the code reaches it.
* Bad, because skif and knip disagree on such repositories, which the harness has to set apart.

### Keep knip's order and document it

* Good, because the counts against knip stay as they are.
* Bad, because a dependency the code never reaches stays listed, and a local file the build compiles is reported
  unused.

### Follow TypeScript, but keep crediting the listed dependency of a mapping into local sources

* Good, because no dependency becomes unused that was used before.
* Bad, because a package that nothing reaches still counts as used, only because a specifier starts with its name.
