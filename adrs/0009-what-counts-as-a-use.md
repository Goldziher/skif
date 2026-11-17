---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# What Counts as a Use

## Context and Problem Statement

The JS rules report an export, member, file or dependency that nothing uses, and an import that no manifest lists or
nothing resolves. Each depends on what counts as a use, and real code uses things in many shapes: a namespace import
whose members are read, a module passed whole to a function, a lazily loaded page, a path handed to
`require.resolve`, a member read through a constant key, a type-only import satisfied by an `@types` package, a
package a config names, an import into generated code that does not exist yet, a name in a Vue template. Counting too
little reports code in use; counting too much hides dead code. knip's answers are the reference, but some of them
depend on executing code or on merged configuration skif does not have.

How does skif decide what uses an export, a class member and a dependency, and how does it read templates and custom
element registrations?

## Decision Drivers

* Each finding has one rule, so severities and suppressions can be set apart
* A use counts for what the source shows it reads; an uncertain read keeps the target used
* A finding gets advice that fixes it in the package that must change
* Each case is settled by what the tool that resolves it does: TypeScript, the bundler, the test runner, the package
  manager
* No case needs configuration to be right

## Considered Options

* Judge each construct where it decides the need, by what its resolving tool does, with its own rule or `detail` where
  the advice differs
* Follow knip's reading everywhere, which depends on executed configs skif does not have
* Count every name match as a use, which hides most dead exports and members
* Require type information for member and export uses, which a measured typed mode made slower and less precise
  ([ADR 0010](0010-type-aware-analysis-on-tsrs.md))

## Decision Outcome

Chosen option: "Judge each construct where it decides the need, by what its resolving tool does, with its own rule or
`detail` where the advice differs", because each case then has a rule that holds beyond one repository, none needs
configuration, and a finding gets advice that fixes it in the package that must change.

### Exports and namespaces

* Each unused export is reported by exactly one rule. A module reached only through `import * as ns` or
  `export * as ns` whose members are read is namespace-only, and its unused exports are reported by
  `unused-namespace-exports` and `unused-namespace-types`, never also by `unused-exports` or `unused-types`.
* A member read is `ns.m`, `ns['m']`, destructuring and `ns.Type` in a type position; a literal `spyOn(ns, 'a')` reads
  `a`; one nested level (`ns.E.A`) is read. References are matched to bindings by scope with `oxc_semantic`, so a
  shadowing local reads nothing of an import.
* A use as a whole (passing, spreading, a computed key, an `import()` or `require()` bound to no names) uses every value
  export and no type export. A namespace the module re-exports itself, entry modules and glob-imported modules use
  every export.
* A function returning `import("x")` passed to a known lazy-component loader (`React.lazy`, `next/dynamic`,
  `@loadable/component`) uses only the default export; passed to any other call it keeps the module used as a whole.
* `require.resolve`, `createRequire(…).resolve` and `import.meta.resolve` use every value export of the module they
  name, as a `require()` that binds nothing does.
* A type that shares its name with a value export is a container like the value. Members read in their own file count
  as used.
* Exempt: exports tagged `@public`, `@beta`, `@alpha` or a tag in `ignore-tags`, exports of entry files unless
  `include-entry-exports`, exports used in the signature of a used export, and registered custom elements. Exports of a
  module inside an incomplete scope are not judged either way
  ([ADR 0008](0008-plugins-config-evaluation-and-commands.md)).

### Class members

* `unused-class-members` judges the members of an exported class that another module uses, in a module whose exports
  are reported. Methods, fields, accessors and ECMAScript private names are members; constructors, `private` members
  and computed keys that are not literals are not.
* Excluded from the declarations: decorated members and classes (a framework may read them by reflection), members of
  a class whose superclass chain leaves the project (except the members a known library base declares), members an
  implemented interface declares, members of classes an entry file exposes, and type-only fields.
* Member usage is name-based, without types ([ADR 0010](0010-type-aware-analysis-on-tsrs.md)): any read of a
  property of that name on any object, in a module or in template markup, credits the member, including an element
  access through a provably constant key resolved across the graph. A plain assignment is no read.
* The fix deletes the member and is unsafe, because a dynamic access may read it; it is withheld where the class
  mentions the name elsewhere, for overloads and for decorated members.

### Dependencies

* `stale-mocks` reports a test-runner mock whose relative or aliased specifier resolves to nothing;
  `unresolved-imports` does not report mocks, so each can be baselined alone.
* A bare builtin name (`events`, `buffer`) uses a listed package of that name, since bundlers resolve it to the
  installed polyfill; a `node:` specifier uses no package.
* An import that an alias resolves into the importer's own sources uses no listed dependency; an aliased import that
  reaches no file still uses the package whose name it starts with. An import only another package's `tsconfig`
  `paths` map is `unlisted-dependencies` with `detail` `ForeignTsconfigPaths`.
* A specifier that looks like a package name and resolves to nothing is a dependency of the package it names, judged
  by the dependency rules rather than `unresolved-imports`; `declare module 'pkg'` covers the imports of that package.
* A type-only import is satisfied by the package or its `@types` package, since it is erased from the output.
* A specifier, `paths` target or alias target that leads into a missing path a `.gitignore` lists names generated code:
  neither unresolved nor unlisted.
* A package a config names as a required input is `unlisted-dependencies` at that config, with `detail`
  `ConfigReference`, against the innermost package holding the config. A package named by an imported module is
  charged to that module's owner, except a bundler alias replacement, which the bundler resolves from the importing
  package. Packages named only on one environment-gated branch are optional references
  ([ADR 0008](0008-plugins-config-evaluation-and-commands.md)).
* The compiler options of a `tsconfig` a package extends through `node_modules` apply to the extending package, as
  TypeScript merges the chain; the installed config itself is never judged.
* `createRequire(location).resolve` names a module of the file only for the file's own location; any other location
  records no import, because the package that resolves it owns the dependency.
* `unused-dev-dependencies` reports a `devDependencies` entry the same manifest lists in `dependencies`.
  `unused-optional-peer-dependencies` counts an optional peer as installed when the package, a containing package or
  the root lists it.
* By default a containing package's or the root's listing satisfies a member's use. `[js] isolate-workspaces` makes
  each package use only its own manifest, and `[js] strict` adds production mode
  ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)).
* Ambient `@types` packages follow the package's TypeScript major and its effective `types` and `typeRoots`: with no
  `types`, TypeScript 6 and later include no `@types` package, so a globals-only one nothing names is unused.

### Templates and custom elements

* Expressions of Vue and Svelte templates are parsed by oxc as TypeScript, and those of Astro and MDX with JSX, under
  a scope stack of the template's binding constructs, so a name a template binds is no use of an import. An expression
  oxc cannot parse falls back to a token scan. Nothing in a template is type-checked.
* A Vue tag uses the names Vue resolves it to. A declaration file generated by `unplugin-vue-components` registers
  components, and each template that names one imports its default export. A package where components may be used
  without a recorded tag keeps the declaration file as an entry that credits every component.
* Marko, Qwik `.mdx` routes and Stencil stylesheets are read by skif's own extractors, so no compiler is needed.
* The parser records each exported class's custom element registrations: `customElements.define`, a registering
  decorator, or a base class's `define`. `define` always counts as a use; decorator and base-class registrations count
  only through a plugin whose `PluginSpec::custom_elements` names the library, so a further library is a plugin row.

### Explaining it

`skif explain-usage FILE#export` and `FILE#Class.member` name the uses found, the exemption or exclusion that applies,
and the verdict. Every new shape of use changes the parse output, so it raises the parse cache schema
([ADR 0007](0007-js-analysis-core-on-oxc.md)).

### Consequences

* Good, because every unused export is reported once, and namespace findings have their own severity.
* Good, because spying on one member, loading modules together or a shadowing parameter hides none of a module's
  other unused exports, and a lazily loaded page has its named exports judged.
* Good, because unused methods and fields are reported without a type checker, and a member read anywhere is never
  reported.
* Good, because configs that name packages no manifest lists are reported where they are written, and a `paths` entry
  into generated code needs no ignore entry.
* Good, because a template local named like an import hides no unused import, and web component libraries do not see
  every element class reported.
* Bad, because common member names are read somewhere in most projects, so unused members with such names are
  missed, and classes of decorator frameworks and unknown bases are not judged.
* Bad, because skif departs from knip where it follows the compiler or the bundler, and a project relying on knip's
  merged `paths` gets findings knip does not report.
* Bad, because an import into a gitignored path that is never generated is not reported, and component registrations
  are global to the run.

### Confirmation

* `scripts/check-rule-ids.sh` and `rule-ids.txt` hold the rule ids.
* `crates/skif-js/tests/namespaces_and_components.rs`, `crates/skif-js/tests/resolve_calls.rs`,
  `crates/skif-js/tests/class_members.rs` and `crates/skif-js-syntax/tests/module_info.rs` cover exports, members and
  their uses; `crates/skif-js/tests/explain_absence.rs` covers the explanation.
* `crates/skif-js/tests/dependency_credits.rs`, `crates/skif-js/tests/config_references.rs`,
  `crates/skif-js/tests/foreign_tsconfig_paths.rs`, `crates/skif-js/tests/installed.rs` and
  `crates/skif-js/tests/isolate_workspaces.rs` cover dependency findings.
* `crates/skif-js-syntax/src/template.rs`, `crates/skif-js/tests/frameworks.rs` and
  `crates/skif-js/tests/custom_elements.rs` cover templates, components and registrations.
