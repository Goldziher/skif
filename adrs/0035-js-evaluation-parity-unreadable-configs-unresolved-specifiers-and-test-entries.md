---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# JS Evaluation Parity: Unreadable Configs, Unresolved Specifiers and Test Entries Follow knip

## Context and Problem Statement

A comparison with knip 6.44.1 on large monorepos shows three places where skif reports findings knip does not. A tool configuration that the static evaluator cannot read (an imported base config, a function with side effects) silently drops the files and packages the tool would have claimed, and only five of the plugins marked their entries incomplete. A specifier that starts like a package name but resolves to nothing, such as a workspace package whose `exports` point at an unbuilt `dist`, is reported as an unresolved import at every use. Test files and the `test`, `tests` and `e2e` directories are entries in every project, with or without a test runner.

## Decision Drivers

* A finding that depends on something the analysis could not read is a false positive
* The same project should give the same findings under knip and skif
* Locked records stay unedited

## Considered Options

* Follow knip in all three places, with a shared default for unreadable configs
* Keep the three behaviours and document them as deviations
* Follow knip for the unresolved specifiers only

## Decision Outcome

Chosen option: "Follow knip in all three places, with a shared default for unreadable configs", because each behaviour produced false positives on real repositories and none of them detected anything knip does not.

* A config that evaluates to an unknown value, has an unknown spread at its top level, or cannot be parsed sets `entries_incomplete` for the plugin that reads it, in the shared resolve path. A plugin opts out with `Plugin::reads_config`, which a purely declarative plugin overrides to `false`. `mergeConfig`, `defu`, `deepmerge`, `merge` and `Object.assign` are evaluated as merges of their object arguments, and an unknown argument contributes an unknown spread.
* A specifier that starts like a package name (`@scope/x`, `pkg/sub`) and resolves to nothing, also through a `paths` entry that points nowhere, is a dependency of the package it names, judged by the dependency rules. A relative specifier that is ignored by Git (also through its extension aliases), or that names a JSON file or a foreign file type (images, fonts, stylesheets, shell scripts, YAML) from a script, is not reported. Stylesheets keep reporting a missing stylesheet import.
* Test files and directories are entries only through a test-runner plugin or `[js] entry`. The exclusion of test files from the project in production mode is unchanged.

A script that runs a binary in a named workspace (`yarn workspace web run biome`, `pnpm --filter web exec vitest`, `npm -w web exec ...`) credits the binary to that workspace as a binary that is used when a provider is listed and never unlisted, unless the workspace has a script of that name.

### Consequences

* Good, because the unresolved-import errors and unused-file warnings that depended on unreadable input disappear.
* Good, because test helpers and legacy specs in projects without a runner are reported, as knip does.
* Bad, because a package whose config cannot be read has no unused-file and unused-dependency findings at all.
* Bad, because an alias that points nowhere for a package-like name is an unlisted dependency instead of an unresolved import.

### Confirmation

* Unit and integration tests cover each case on synthetic fixtures (`fp_config.rs`, `config_functions.rs`, `graph_fixes.rs` and the resolver tests).
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Follow knip in all three places, with a shared default for unreadable configs

* Good, because the findings agree with the reference tool.
* Bad, because a convention that helped projects without a runner is gone.

### Keep the three behaviours and document them as deviations

* Good, because nothing changes.
* Bad, because the false positives stay.

### Follow knip for the unresolved specifiers only

* Good, because it fixes the largest error count.
* Bad, because the other two remain silent false positives and false negatives.

## More Information

* Amends: [ADR 0010](0010-js-analysis-on-oxc.md)
* See the [ADR index](index.md).
