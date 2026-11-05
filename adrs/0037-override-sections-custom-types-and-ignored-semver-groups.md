---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Override Sections, Custom Types, the Local Type and Ignored Semver Groups: Completing the syncpack Dependency Types

## Context and Problem Statement

[ADR 0009](0009-version-and-catalog-policies.md) fixes skif's version and semver groups to syncpack's, and [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md) names the default scope: `dependencies`, `devDependencies` and the package's own version. The dependency types of syncpack go further than that. `resolutions`, `overrides` and `pnpmOverrides` are built-in types, `customTypes` makes any manifest field a type (`engines.node`, `packageManager`), `local` selects a package's own version, and a semver group with `isIgnored` or `isDisabled` needs no range. skif had none of these: a version group that pins or bans a dependency passed silently when the dependency sat in `resolutions` or `overrides`, `engines.node` could not be synced across packages, a `local` selector was rejected, and a semver group without a `range` did not parse. The sherif port also lacked three of its ten built-in dependency families and part of the Storybook list.

## Decision Drivers

* A syncpack configuration that relies on these types must keep its meaning, or be refused with a clear message
* The default scope of ADR 0030 must not change under existing users
* Edits go through the existing JSON and YAML fix operations
* One instance model: a version in an override section or a custom type is an instance like a dependency entry, so every policy applies to it

## Considered Options

* Extend the instance model with override sections and custom types, keep the default scope, and add `local`, ignored semver groups and the missing families
* Govern every type by default, as syncpack does
* Leave the types out and document them as not supported

## Decision Outcome

Chosen option: "Extend the instance model with override sections and custom types, keep the default scope, and add `local`, ignored semver groups and the missing families", because it removes the silent passes without moving the default scope that ADR 0030 recorded.

**Dependency types.** `dependency-types` (in `[policy]`, `[[version-groups]]` and `[[semver-groups]]`) names the built-in types `prod`, `dev`, `peer`, `optional`, `local`, `resolutions`, `overrides` and `pnpm-overrides` (syncpack's `pnpmOverrides` is accepted) and the name of any `[[custom-types]]` entry, each optionally negated with `!`. `local` is a package's own version, which was an instance already; the token can now be named, so `["local"]` selects it and `["!local"]` excludes it. A token that is neither built-in nor a defined custom type fails when the configuration loads, with the closest name suggested.

**Default scope.** The override sections and custom types follow peer and optional: a configured group that names the type governs it, and the catch-all group takes it only when `[policy] dependency-types` names it. This keeps ADR 0030 (production and development dependencies plus the own version) and differs from syncpack, which governs all of them by default. A positive `[policy] dependency-types` list that omits `prod` and `dev` still drops them, as before.

**Override sections.** `resolutions` (yarn), `overrides` (npm, with nested objects) and `pnpm-overrides` are read from every active package. `pnpm-overrides` covers `pnpm.overrides` of `package.json` and `overrides` of `pnpm-workspace.yaml`; syncpack's `pnpmOverrides` is the YAML mapping only, and `pnpm.overrides` is reachable there through a custom type. The dependency of an entry is the package its selector names: `react@<18`, `parent>react`, `**/react`, `parent/**/react` and `@scope/pkg@1` are instances of `react` and `@scope/pkg`; syncpack takes the key as written, so these never align with the plain dependency. In an npm nested object the `.` entry belongs to the parent. Only string values are instances. A JSON entry is located and fixed in `package.json`; a YAML entry is located by a scan of the `overrides:` block of `pnpm-workspace.yaml` (block mappings; a flow mapping has no span) and fixed with a YAML operation.

**Custom types.** `[[custom-types]]` has `name`, `path` (dotted path of keys in `package.json`) and `strategy`: `version` (the value is a version string, the dependency is named after the type: `engines.node`), `name@version` (the value is `name@version`, the dependency is the name: `packageManager`) and `versions-by-name` (an object of `name: version`; `versionsByName` is accepted). The `name~version` strategy and `source` of syncpack are not supported. A name must not be a built-in token and must be unique. Entries merge by name across layered files, `reset = ["custom-types"]` discards the inherited ones, and an override table cannot carry them. A fix rewrites the string in place and keeps the `name@` prefix.

**Ignored semver groups.** A `[[semver-groups]]` entry takes exactly one of `range` and `ignored = true`. An ignored group matches like any other, requires no range, and stops later groups from applying to what it matched; syncpack's `isIgnored` and `isDisabled` both map to it.

**Built-in families (sherif).** `turborepo` (`turbo`, `turbo-ignore`, `@turbo/gen`, `eslint-config-turbo`, `eslint-plugin-turbo`), `eslint-stylistic` (`@stylistic/eslint-plugin*`) and `lexical` (`lexical`, `@lexical/*`) are added, the Storybook list is the union of skif's and sherif's, and `prisma` covers the adapters. Where skif's family was a glob (`@trpc/*`, `@next/*`, `@typescript-eslint/*`) it stays a superset of sherif's list. A unit test checks every package of sherif 1.13's lists against the families.

**Deferred.** `specifierTypes` is already supported. Not done here: syncpack's `dependencyGroups`, `semverRangeOnly`, `severity`, `source`/`sourceMode`, the `name~version` strategy, catalog types, and per-state severities.

### Consequences

* Good, because pinning, banning and aligning work on `resolutions`, `overrides` and pnpm overrides, and `engines.node` or `packageManager` can be synced, by the same groups as dependencies.
* Good, because the default scope is unchanged for configurations that name none of the new types.
* Bad, because the types are opt-in, so a syncpack configuration that relies on the default needs `dependency-types` listed, which a migration tool has to emit.
* Bad, because the dependency of an override entry differs from syncpack's for selectors with a parent or a range.

### Confirmation

* `fixtures/policy/dependency-types` (12 cases), `semver-groups/ignored-group-exempts-dependency` and four `dependency-families` cases run in the policy conformance suites, with fixes and the idempotent re-run.
* Unit tests cover the token parser, the validation of custom types and ignored groups, layering, the field collector, selector names, the YAML override locator and the sherif family list.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Extend the instance model with override sections and custom types, keep the default scope, and add `local`, ignored semver groups and the missing families

* Good, because one instance model serves every policy and every fix.
* Good, because existing configurations and the corpus comparison keep their scope.
* Bad, because the new types need an explicit `dependency-types`.

### Govern every type by default, as syncpack does

* Good, because it is syncpack's behaviour.
* Bad, because it changes what existing configurations check: a custom type or an override section would join the catch-all group of every user at once.

### Leave the types out and document them as not supported

* Good, because it needs no code.
* Bad, because the silent pass on pinned and banned dependencies stays, and `engines.node` stays unsyncable.

## More Information

* Extends: [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md)
* See the [ADR index](index.md).
