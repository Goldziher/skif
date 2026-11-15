---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Packages Another Package Resolves Are That Package's Dependencies

## Context and Problem Statement

[ADR 0059](0059-resolve-calls-use-the-values-of-the-module.md) records `createRequire(…).resolve(specifier)` as a
resolve call of the file it is written in: the package the specifier names is a dependency of that file's package.
`createRequire(location)` builds a `require` that resolves from `location`, which is this file only when it is given
as `import.meta.url`, `import.meta.filename` or `__filename`. A test that reads a data file of a package another
workspace member installs writes:

```js
const designSystem = join(__dirname, '../../design-system');
createRequire(join(designSystem, 'package.json')).resolve('icon-set/json/lucide.json');
```

Node.js resolves `icon-set` from the design system's directory, where its manifest lists it. skif reported it as an
unlisted dependency of the test's package.

The Heft Storybook plugin has the same shape. Its task options name a storykit package and a CLI package; the plugin
resolves the storykit from the project and then the CLI package from the storykit's folder. skif required both from
the project, so a project whose storykit lists the CLI package got an unlisted finding for it.

When another package resolves a specifier, whose dependency is the package it names?

## Decision Drivers

* A finding follows how the runtime or the tool resolves the specifier
  ([ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md))
* A finding names the manifest that must change
* A listing in the project that the tool does not need is no reason to report the project

## Considered Options

* The package that resolves the specifier owns the dependency
* Keep judging every specifier against the package of the file or config that names it

## Decision Outcome

Chosen option: "The package that resolves the specifier owns the dependency", because only that package's manifest
decides whether the specifier resolves.

* The `resolve` of a `createRequire` call names a module of the file only when the call is given `import.meta.url`,
  `import.meta.filename` or `__filename`. Given anything else (a path built with `join`, a variable, `__dirname`,
  `import.meta.dirname`), the call records no import: the specifier is neither a dependency of the file's package nor
  a file it keeps in use. This supersedes ADR 0059 for such calls; a `require` bound to `createRequire(…)` and the
  other resolve calls are unchanged.
* The `cliPackageName` of a `@rushstack/heft-storybook-plugin` task is a package the config may name without the
  project listing it: a listing in the project is used, a missing one is not reported. The `storykitPackageName` stays
  a package the project needs.

### Consequences

* Good, because a test or script that resolves a file through another package's install is not told to list that
  package's dependencies.
* Good, because a Storybook task is reported only for the storykit the project must install.
* Bad, because `createRequire(pathToFileURL(__filename))` or a variable holding this file's URL counts as another
  location, so the package its `resolve` names is not credited.
* Bad, because a `require` bound to `createRequire(location)` of another package still resolves from this file in
  skif's view.

### Confirmation

* `crates/skif-js-syntax/tests/module_info.rs`: `a_create_require_resolve_names_a_module_only_from_this_file`.
* `crates/skif-js/tests/unlisted_parity.rs`: `a_create_require_of_another_package_resolves_that_packages_dependencies`.
* `crates/skif-js/tests/heft_rigs.rs`: `the_storybook_cli_package_is_a_dependency_of_the_storykit` and
  `heft_tasks_credit_their_tests_and_option_packages`.

## Pros and Cons of the Options

### The package that resolves the specifier owns the dependency

* Good, because the finding matches what the runtime and the tool resolve.
* Bad, because only the spellings of this file's location that the parser knows keep the call an import.

### Keep judging every specifier against the package of the file or config that names it

* Good, because nothing changes.
* Bad, because the advice adds a dependency the package does not need, and removing it from the package that does
  need it would go unreported.
