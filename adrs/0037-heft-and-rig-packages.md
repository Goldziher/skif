---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Heft and Rig Packages: Profiles as Configs of the Rig and of the Projects That Name It

## Context and Problem Statement

In a Rush repository ([ADR 0012](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md)) the projects build with
Heft and take their build configuration from a rig package. A project's `config/rig.json` names the rig
(`rigPackageName`) and one of its profiles (`rigProfile`, `default` unless set); Heft and its plugins look for each of
their config files in the project's `config` folder first and then in the rig's `profiles/<profile>/config` folder. A
rig's profile `config/heft.json` extends another rig's
(`"extends": "@rushstack/heft-node-rig/profiles/default/config/heft.json"`) and names the plugin package of every task
and lifecycle plugin (`"pluginPackage": "@rushstack/heft-sass-plugin"`), and its `profiles/<profile>/tsconfig-base.json` is the base the projects' `tsconfig.json` extends. The plugin table of
[ADR 0015](0015-plugin-model-and-plugin-set.md) had no Heft plugin and read no file below `profiles`, so every
dependency a rig lists for its profiles (`@rushstack/heft`, the rig it extends, the plugins it loads, `sass`) was
reported unused. How do Heft's config files, `config/rig.json` and a rig's profiles count?

## Decision Drivers

* A rig's profiles are the only files that use what the rig lists: it has no source of its own
* A project that names a rig in `config/rig.json` builds with it, so it needs the rig
* Config files are read statically and never run (ADR 0015)
* A compiler option of a shared base `tsconfig` is satisfied by the base's owner or by the package extending it
  ([ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md)), and a rig is such an owner
* A package a plugin loads at run time without the project naming it is credited, never required, as knip's optional
  references are

## Considered Options

* A `heft` plugin that reads Heft's config files, `config/rig.json` and a rig's own profiles, with a rig's profile
  `tsconfig` files shared with the projects that extend them
* Read a rig's profiles only from the projects that name the rig, never in the rig itself
* No plugin: rigs list their dependencies in `ignore-dependencies`

## Decision Outcome

Chosen option: "A `heft` plugin that reads Heft's config files, `config/rig.json` and a rig's own profiles, with a rig's
profile `tsconfig` files shared with the projects that extend them", because the rig is the package whose manifest
lists what its profiles name, and the projects that use it are known only through files in their own folders.

### The plugin

* `heft` is enabled by a dependency on `@rushstack/heft` or by a `config/rig.json`, and provides the `heft` binary. It
  has no knip counterpart.
* Its configs are `config/heft.json`, `config/rig.json`, `config/typescript.json`, `config/jest.config.json` and, in a
  rig, `profiles/*/config/*.json`, read as JSON with comments.
* The `extends` of each of them names a further config when relative, else the package it lies in, which the config
  needs. The `pluginPackage` of every lifecycle plugin (`heftPlugins`) and every task plugin
  (`phasesByName.<phase>.tasksByName.<task>.taskPlugin`) of a `heft.json` is a package the config needs.
* A plugin package that loads a package the project provides credits that package without requiring it:
  `@rushstack/heft-sass-plugin` credits `sass`, so a rig that lists `sass` uses it and one that does not is not told
  to list it.

### `config/rig.json`

* `rigPackageName` is a package the project needs. A value that is no package name names nothing.
* The selected profile of the installed rig, `node_modules/<rigPackageName>/profiles/<rigProfile>`, holds configs of
  the project: each `config/*.json` the project has no file of the same name for in its own `config` folder, read by
  `heft`, and each `tsconfig*.json`, read by `typescript`. A `rigProfile` that is not one directory name (`../x`)
  selects no profile. A rig that is not installed gives only the credit of the rig itself.
* The files lie in `node_modules`, so the packages they name are credits of the project and never judged at the file,
  as for any installed config.

### A rig's own profiles

* In a rig, `profiles/*/config/*.json` are its own configs, so what their `extends` and `pluginPackage` name are its
  dependencies, and a profile `heft.json` credits `@rushstack/heft`, which the rig provides to the projects that use
  it, without requiring it.
* `profiles/*/tsconfig*.json` are configs the rig ships for the projects that extend them, a new plugin input
  (`SharedConfig`): `typescript` reads them and what they extend, a package their `extends` path names into
  `node_modules` is the rig's dependency, and the packages their compiler options name (`types`, `importHelpers`,
  `jsxImportSource`, `plugins`) are credits of the rig, never unlisted at the rig. Their `paths`, `baseUrl`, output
  directories and `customConditions` describe the projects and are left out.
* So `importHelpers` in a rig profile is satisfied by the rig's manifest or by the projects that extend the profile,
  as ADR 0019 has it for a base config's owner and its extenders; a project whose own `tsconfig` sets it is judged as
  before.

### Consequences

* Good, because a rig's dependencies are used exactly where its profiles name them, and one nothing names is still
  reported unused.
* Good, because a project that lists its rig only for `config/rig.json` uses it.
* Good, because the reading is static: no rig is resolved by running `@rushstack/rig-package`, and an uninstalled rig
  costs only the profile configs.
* Bad, because a rig that is a workspace member but not installed yet contributes no profile configs to the projects
  naming it: they are found through `node_modules` only.
* Bad, because `tslib` for a rig profile's `importHelpers` is never unlisted at the rig, so a project that extends the
  profile without listing `tslib` is reported only when its config is judged, not through the rig.
* Bad, because a Heft plugin that loads a package by name credits it only once it is added to the plugin's list.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs`: `heft_config_names_extended_and_plugin_packages`,
  `heft_rig_json_credits_the_rig_and_reads_its_profile`, `heft_rig_json_with_a_path_as_profile_reads_no_profile` and
  `heft_rig_profiles_are_the_rigs_own_configs`.
* `crates/skif-js/tests/heft_rigs.rs`: `rig_profiles_credit_the_rigs_dependencies_and_rig_json_the_rig` and
  `a_rig_no_rig_json_names_is_unused`.

## Pros and Cons of the Options

### A `heft` plugin that reads Heft's config files, `config/rig.json` and a rig's own profiles, with a rig's profile `tsconfig` files shared with the projects that extend them

* Good, because the manifest that lists a profile's dependencies is the one judged for them.
* Good, because a project's config wins over its rig's, as Heft resolves them.
* Bad, because the plugin input model grows a shared config besides a linked one.

### Read a rig's profiles only from the projects that name the rig, never in the rig itself

* Good, because the profiles are read as Heft reads them, from a project.
* Bad, because what the profiles name would be credited to the projects, which do not list it, and the rig's own
  dependencies would stay unused.

### No plugin: rigs list their dependencies in `ignore-dependencies`

* Good, because nothing changes in the analysis.
* Bad, because every Rush repository has to configure the same exceptions, and a dependency a rig no longer needs is
  never reported.
