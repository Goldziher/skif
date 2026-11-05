---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Monorepo JS Corrections: Builtins, HTML Entries, Referenced Paths, Extglob, Config Helpers, Stylesheet Lookups and Per-Package Extensions

## Context and Problem Statement

Running the JS analysis on a large pnpm monorepo found seven places where skif disagrees with knip on constructs that real workspaces use: `import 'buffer/'` is taken for the Node.js builtin, the vitest plugin ignores `index.html` when the vite plugin is off, the `paths` of a referenced `tsconfig` apply to every package, `(a|b)` extglob groups in plugin globs match nothing, a shared config helper in a sibling package makes the package that defines it look unused, stylesheet imports through `baseUrl` or `paths` are resolved as scripts, and the `resolve.extensions` of one package apply to all.

## Decision Drivers

* Follow knip, which evaluates configs and uses picomatch and the TypeScript and bundler resolution rules
* A finding that is wrong in one package must not be hidden in all of them
* Nothing is executed; what cannot be read statically must make the affected packages conservative, not the workspace

## Considered Options

* Fix each case at the place that decides it, with the narrowest scope that matches knip
* Keep the global behaviour and add ignores to the configuration
* Execute configs to get exact values

## Decision Outcome

Chosen option: "Fix each case at the place that decides it, with the narrowest scope that matches knip", because it removes the false findings without hiding real ones and keeps the analysis static.

**Builtins.** A specifier is a builtin when it has the `node:` prefix or equals a builtin name or one of the builtin subpaths (`fs/promises`, `path/posix`, `stream/web`, and the other names of `module.builtinModules` that contain a `/`). `buffer/` and `events/` name the npm packages, which is Node's rule.

**HTML entries.** The vitest plugin adds the `index.html` scripts and the `build.rollupOptions.input` entries of its config, as knip's vitest plugin does by reusing the vite plugin's handling. It does so whether or not the vite plugin is enabled.

**Referenced `tsconfig` paths.** The `baseUrl`, `paths`, output directories and conditions of a config reached through `references` that lies in another workspace package (a sub-directory with its own `package.json`) are not given to the package that references it. They apply to that package, whose own run reads its config, and the nearest `tsconfig.json` still applies them to its files. A referenced config in the same package keeps contributing.

**Extglob.** Plugin and configuration globs accept picomatch's groups: `(a|b)` and `@(a|b)` become `{a,b}`, `?(a|b)` and `*(a|b)` become `{a,b,}`, `+(a|b)` becomes `{a,b}` and `!(a|b)` becomes `*`. The last two are approximations (one occurrence, and a superset), chosen so that a source file is never missing from the entries. A group without `|` stays literal, so Next.js route groups such as `app/(group)` keep naming a directory.

**Config helpers.** A config that evaluates to something unknown still marks its own package's entries incomplete. skif also records the modules the config imports; a relative import that lies in another workspace package, or a bare import of a workspace package, marks that package incomplete as well and makes all of its project files entries. A helper may name files (`path.join(here, 'global-setup.ts')`) and packages that only it knows, and what it names cannot be read; treating the defining package as used credits those files and their dependencies, and only the packages a helper is imported from are affected.

**Stylesheet lookups.** Aliases, `paths`, `baseUrl` and `resolve.modules` are looked up for a specifier written in a stylesheet with the stylesheet resolver, and also as a Sass partial (`_name`) or an index partial, instead of with the script resolver.

**Bundler extensions.** The `resolve.extensions` a bundler config declares apply to the files of the package whose config declares them (the innermost package that holds the importer), not to the workspace.

### Consequences

* Good, because each case reports what knip reports on the same input.
* Good, because the helper rule is limited to the packages a helper is imported from.
* Bad, because a package that defines a config helper is no longer judged for unused files and dependencies.
* Bad, because `+(…)`, `*(…)` and `!(…)` are approximations.
* Bad, because a bundler config in a root package no longer widens the extensions of the packages below it.

### Confirmation

* `crates/skif-js/tests/monorepo_regressions.rs` has one workspace per case, taken from the minimal reproductions.
* Unit tests cover `is_builtin` and the extglob translation; a catalog test pins that a group with a negated dependency type claims the instance.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Fix each case at the place that decides it, with the narrowest scope that matches knip

* Good, because findings are right per package.
* Bad, because the helper rule is an approximation of evaluating the helper.

### Keep the global behaviour and add ignores to the configuration

* Good, because it changes no code.
* Bad, because every repository needs the same ignores for a tool default that is wrong.

### Execute configs to get exact values

* Good, because values are exact.
* Bad, because skif never runs project code.

## More Information

* Refines: [ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md)
* See the [ADR index](index.md).
