---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Publicly Hoisted Packages as Available Dependencies

## Context and Problem Statement

`unlisted-dependencies` reports a file that imports a package which neither its `package.json` nor any manifest above
it lists. pnpm's `public-hoist-pattern` and `shamefully-hoist`, and `public_hoist_packages` of rules_js's
`npm_translate_lock`, hoist matching packages into the root `node_modules` (rules_js: into the `node_modules` of each
Bazel package listed), where Node.js resolves them from every package below without a listing. A repository that
relies on that, with `public-hoist-pattern[]=hoisted-only` in `.npmrc` or
`public_hoist_packages = {"hoisted-only": [""]}`, gets `unlisted-dependencies hoisted-only` for every package that
imports it. knip reports the same, so the finding is parity, but the repository has to list each name in
`ignore-dependencies` (GitHub issue #178).

Should skif count a publicly hoisted package as available, and from which settings?

## Decision Drivers

* The default keeps parity with knip: a hoisted package that a package uses is still a dependency it does not declare
* A repository that hoists on purpose can say so once instead of naming every package
* Only settings committed with the repository are read; an analysis must not depend on the user's machine
* The settings are read where the package manager reads them, as for pnpm's linker (ADR 0108)

## Considered Options

* An opt-in `[js] use-public-hoisting` that reads pnpm's public hoist settings and rules_js's `public_hoist_packages`
* Count publicly hoisted packages as available by default
* Leave hoisted packages to `ignore-dependencies`

## Decision Outcome

Chosen option: "An opt-in `[js] use-public-hoisting` that reads pnpm's public hoist settings and rules_js's
`public_hoist_packages`", because it keeps knip's answer by default and lets a repository that hoists on purpose
take the hoist as the listing it stands in for.

`use-public-hoisting` is a boolean of the top-level `[js]` table, `false` by default. It cannot be set in an override
or a `[js.workspaces]` entry: hoisting is a fact of the install, which is shared by the whole root. With it on and
outside strict mode, an import, or a package a tool config names, that no manifest of the package or above lists is
satisfied when the package is hoisted into a `node_modules` at or above the importing package:

* Under pnpm (not a Rush repository), `shamefullyHoist` and `publicHoistPattern` of `pnpm-workspace.yaml` decide,
  else `shamefully-hoist` and `public-hoist-pattern` of the top level of the root `.npmrc`, which is read only when
  the root's `packageManager` names a pnpm before 11 or no pnpm version, as for the linker in ADR 0108. The `.npmrc`
  list is built as npm's `ini` parser builds it: a `public-hoist-pattern[]` entry appends and makes the value a list,
  a plain entry appends to a list and replaces a single value. `shamefully-hoist` true is the pattern `*`, as in pnpm.
  The patterns are matched as pnpm's matcher matches them: `*` matches any run of characters and anything else
  itself, a `!` pattern excludes what the patterns before it include, and patterns that all exclude include
  everything else. They hoist into the root `node_modules`, so they hold for every package. Without a setting no
  package is hoisted: pnpm's default changed from `*eslint*` and `*prettier*` to none in pnpm 10, and the version is
  often not declared.
* In a Bazel repository, every `public_hoist_packages = {...}` dict literal of the repository root's `MODULE.bazel`,
  `WORKSPACE.bazel` or `WORKSPACE`, as a keyword argument or an assignment, hoists each string key, a version
  suffix (`@1.2.3`) dropped, into the directory of each Bazel package that its list literal names (`""` is the
  repository root); the package is available to the packages at or below it. A computed key or list names nothing,
  and `.bzl` files are not read.

A hoisted package that a manifest lists is found through the listing first, so `unused-dependencies` and every use
count are unchanged: hoisting satisfies an unlisted import and makes nothing used or unused. Strict mode, in which a
package uses only its own manifest, ignores the setting. Binaries are not affected: as ADR 0108 decides, an external
binary is credited only to a package that lists the dependency providing it. `node-linker=hoisted` and the flat
layouts of npm, Yarn Classic and Bun are not counted: they make every installed package resolvable from every
package, and counting them would turn `unlisted-dependencies` off, which a rule level already does. Whether a matched
package is installed is not checked; a pattern that matches a package the repository never installs satisfies its
import too. knip has no such setting, so `skif migrate` has nothing to translate.

### Consequences

* Good, because a repository that hoists packages on purpose states it once and keeps `unlisted-dependencies` on for
  everything else.
* Good, because the default and strict mode keep knip's answer.
* Bad, because a hoist that only the user's `.npmrc`, the environment, the command line or a `.bzl` file sets is not
  seen.
* Bad, because with `shamefully-hoist` the setting satisfies every unlisted import, also of a package that is not
  installed.

### Confirmation

* `crates/skif-js/tests/public_hoisting.rs`: `pnpm_public_hoist_settings_make_packages_available_when_on` (off, exact,
  scoped and wildcard patterns, a `!` pattern after `*` and on its own, plain and array `.npmrc` entries combined as
  `ini` combines them, `shamefully-hoist` true and false, a commented-out line, one under a section, a member's
  `.npmrc`, no setting, `pnpm-workspace.yaml` overriding `.npmrc`, pnpm 11 ignoring `.npmrc` but not
  `pnpm-workspace.yaml`, and npm), `bazel_public_hoist_packages_make_packages_available_where_they_are_hoisted`
  (off, the root, a versioned key hoisted into one package and not another, `WORKSPACE`, a commented-out call),
  `strict_mode_does_not_count_hoisted_packages` and `hoisting_leaves_unused_dependencies_as_they_are`.
* `crates/skif-js/src/hoisting.rs`: the matcher, `.npmrc` list and flag, and version suffix tests.
* `crates/skif-workspace/src/pnpm.rs`: `public_hoisting_settings_are_read_when_well_typed`.
* `crates/skif-js-plugins/src/bazel.rs`: `public_hoist_packages_are_the_literal_keys_and_locations`.
* `crates/skif-config/tests/config.rs`: the key parses, defaults to `false` and is rejected in an override.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### An opt-in `[js] use-public-hoisting` that reads pnpm's public hoist settings and rules_js's `public_hoist_packages`

* Good, because the answer follows the hoist the repository configures, in the files its tools read.
* Good, because the default is unchanged.
* Bad, because one more key and two more settings to read.

### Count publicly hoisted packages as available by default

* Good, because a hoisting repository needs no configuration.
* Bad, because it departs from knip, and a package that relies on a hoist breaks when it is installed on its own.

### Leave hoisted packages to `ignore-dependencies`

* Good, because nothing changes.
* Bad, because every hoisted name needs an entry, which also keeps the package from ever being reported as unused
  and goes stale when the hoist changes.

## More Information

* Issue: [#178](https://github.com/Goldziher/skif/issues/178)
* Related: [ADR 0108](0108-member-binaries-under-pnpm-s-hoisted-node-linker.md) (where pnpm's settings are read)
* pnpm settings: <https://pnpm.io/settings#publichoistpattern>, <https://pnpm.io/settings#shamefullyhoist>
* rules_js `npm_translate_lock`: <https://github.com/aspect-build/rules_js/blob/main/docs/npm_translate_lock.md>
* See the [ADR index](index.md).
