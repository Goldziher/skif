---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Redundant Development Entries, Optional Peers Installed Elsewhere and Types an Installed Config Brings

## Context and Problem Statement

`unused-dev-dependencies` reports a `devDependencies` entry that nothing uses, and `unused-optional-peer-dependencies`
reports an optional peer dependency the code uses while the package does not install it. On the pinned corpus four
cases fall outside what these rules, [ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md) and
[ADR 0052](0052-generated-paths-alias-replacements-and-installed-compiler-options.md) say:

* A package listed in both `dependencies` and `devDependencies` of one manifest. The development entry has no effect,
  since the production entry installs the package for development too. skif counted both entries as used; knip
  reports the development entry.
* An optional peer that the root or a containing package lists in `dependencies` or `devDependencies`. By default a
  containing package's or the root's listing satisfies a member's use (ADR 0019, *Workspace isolation*), but the
  rule asked only the package's own manifest and advised adding what the root already installs.
* An optional peer that a bundler keeps external (tsdown `deps.neverBundle`). The plugin names it as a reference that
  may be absent, which put it on the plugin's ignore list, and that list silenced the rule. Keeping a package out of
  the bundle does not install it for the tests.
* The `types` of a `tsconfig` that a package extends through `node_modules` (a rig profile) whose own `typeRoots`
  point at the rig's install (`../../node_modules/@types`). ADR 0052 makes every compiler option of such a config a
  use of the extending package, so an optional peer `@types/node` became used and was reported as not installed,
  while TypeScript loads those types from the rig.

## Decision Drivers

* A finding follows how the package manager and the compiler resolve the construct (ADR 0019)
* A finding gives advice that fixes it, in the manifest that must change
* The knip comparison stays explainable: a difference is a skif fix, a named deviation or a knip false finding
* An accepted record is not edited; a changed decision is a new record

## Considered Options

* Decide each case by what the package manager and TypeScript install and load
* Keep the rules as they are and leave the cases to `ignore-dependencies`
* Follow knip in each case

## Decision Outcome

Chosen option: "Decide each case by what the package manager and TypeScript install and load", because each rule
holds for every repository with the construct and none needs configuration.

### Development entries a production entry makes redundant

* `unused-dev-dependencies` reports a `devDependencies` entry of a package that the same manifest lists in
  `dependencies`, used or not: "`<name>` is listed in `devDependencies` and in `dependencies`", with the advice to
  remove it from `devDependencies` and the unsafe fix that removes it. One that nothing uses is reported as unused, in
  both sections, as before.
* A package that is not judged (excluded, or with incomplete entries) and an `ignore-dependencies` entry leave it
  unreported, as for an unused one. A listing in `optionalDependencies` or `peerDependencies` beside the development
  entry is no duplicate: neither installs the package for development in every case.

### Optional peers installed elsewhere

* `unused-optional-peer-dependencies` counts an optional peer as installed when the package, a package whose
  directory contains it or the root lists it in `dependencies`, `devDependencies` or `optionalDependencies`. With
  `isolate-workspaces` (and so with `strict`) only the package's own manifest counts.
* Only the `ignore-dependencies` lists of the configuration silence the rule. A reference a plugin names as one that
  may be absent (a package a bundler keeps external) does not.

### Types an installed config brings

* A `types` entry of an installed config (one below a `node_modules` directory) names no package of the extending
  project when the first of that config's `typeRoots` that holds the types is one of the config's own install, written
  relative to the config. TypeScript loads them from there, so they come with the config's package. This supersedes,
  for such `types` entries, the rule of ADR 0052 (*Compiler options of installed configs*) that makes the package a
  use of the extending package.
* A root written with `${configDir}` is the project's own: types it holds first stay a use of the project's listing.
  A config without `typeRoots`, or whose roots hold nothing (no install), keeps ADR 0052's rule.

### Not adopted: a webpack config credits webpack and webpack-cli

* knip's webpack plugin credits `webpack-cli` as an optional dependency of every webpack config it loads, and `webpack`
  through its peer-host rule, since `webpack-cli` declares `webpack` as a peer. skif keeps judging both by the scripts,
  commands and imports that run them; a package whose webpack config a command with a computed working directory runs
  reports them unused. This is a named difference of the knip comparison.

### Consequences

* Good, because a redundant development entry is reported with advice that removes it, as knip reports it.
* Good, because an optional peer that the root installs is no longer reported in each member that imports it.
* Good, because a package keeping an optional peer out of its bundle still learns that its tests need it installed.
* Good, because a project that takes its types from a rig profile is not told to install them itself.
* Bad, because `skif explain-usage --dependency` describes the first listing of a package listed twice, which is the
  production one, as used.
* Bad, because an optional peer the root installs passes for a member published on its own, whose consumers must
  still install it.
* Bad, because without an install skif cannot see what a rig's type roots hold, and keeps ADR 0052's rule.

### Confirmation

* `crates/skif-js/tests/dependency_credits.rs`: `a_dev_dependency_the_manifest_also_lists_in_dependencies_is_reported`.
* `crates/skif-js/tests/installed.rs`: `an_optional_peer_the_root_installs_is_installed_for_the_member`.
* `crates/skif-js/tests/config_references.rs`:
  `types_an_installed_config_finds_in_its_own_type_roots_come_with_its_package` and
  `an_optional_peer_a_bundler_keeps_external_is_reported_unless_the_configuration_ignores_it`.
* `crates/skif-js-plugins/tests/plugins.rs`:
  `typescript_types_an_installed_config_finds_in_its_own_type_roots_name_no_package`.

## Pros and Cons of the Options

### Decide each case by what the package manager and TypeScript install and load

* Good, because each finding names the manifest that must change and the change that fixes it.
* Bad, because ADR 0052's rule for installed compiler options gains an exception.

### Keep the rules as they are and leave the cases to `ignore-dependencies`

* Good, because nothing changes.
* Bad, because every member importing a root-installed optional peer needs an ignore entry, and a redundant entry is
  never reported.

### Follow knip in each case

* Good, because the comparison has fewer differences.
* Bad, because knip judges an optional peer against the package's own `dependencies` and `devDependencies` only, so
  it reports one that `optionalDependencies` installs, and it credits `webpack` for any webpack config.
