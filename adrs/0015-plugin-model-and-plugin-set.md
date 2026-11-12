---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Plugin Model and Plugin Set: A Declarative Table With Optional Static Resolvers

## Context and Problem Statement

Without knowledge of the tools a project uses, the JS analysis reports their config files as unused, the packages
they load as unused dependencies and the binaries their scripts run as unlisted. knip solves this with plugins: each
knows a tool's config files, entry patterns and how to read its config, mostly by loading and executing the config.
skif does not execute configuration ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)), and the
set of tools is large, follows each tool's releases, and grows with what users run.

How is a plugin expressed, when is it enabled, what may it read, how does the set grow, and where may a plugin depart
from knip?

## Decision Drivers

* Most plugins are a few lines of data, reviewable without reading code, and free at run time unless enabled
* A plugin whose value is in its config reads it statically, never by executing it
* Plugins share extraction helpers instead of each walking the AST
* Every plugin is justified by real use and pinned by a test
* Plugin ids match knip's, so configuration and comparison carry over
* A deviation from knip never reports a file or dependency in use

## Considered Options

* A declarative table behind one trait, with optional static resolvers and shared extraction helpers
* One hand-written resolver per plugin
* User-defined plugins in configuration
* Execute tool configs as knip does

## Decision Outcome

Chosen option: "A declarative table behind one trait, with optional static resolvers and shared extraction helpers",
because most plugins stay data, the resolver hooks cover config-driven tools without executing anything, and the shared
helpers keep each resolver short.

### The model

* A plugin is a row of the table in `skif-js-plugins` behind the `Plugin` trait: `spec()` returns the declarative
  `PluginSpec`, and the optional hooks are `resolve_config` (knip's `resolveFromAST`, on the statically evaluated
  config), `resolve_workspace`, `resolve_without_config`, `read_script` (a config that is a script calling the tool's
  API), `incomplete_scope` ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)) and `reads_config`.
* `PluginSpec` holds the id, enablers, `root_only`, `config`, `entry`, `production` and `project` patterns, a dotted
  `package_json_key`, the binaries the tool provides, ignore patterns, custom element registrations
  ([ADR 0020](0020-templates-components-and-custom-elements.md)), `optional_binaries` and the knip commit the row was
  checked against.
* A resolver returns `PluginInput`s, a `#[non_exhaustive]` vocabulary: entries and production entries (also as regular
  expressions), dependencies, optional dependencies, binaries, config files, scripts (also run in another directory or
  found in a source file), aliases, build directories, conditions, extensions, linked configs another plugin reads,
  virtual modules the tool resolves at build time, incomplete entries, and a replacement for the default entry globs.
* The table is the source of truth: `crates/skif-js-plugins/src/plugins` and `registry.rs`. The test list `ADR_IDS` in
  `crates/skif-js-plugins/tests/plugins.rs` and `scripts/check-plugin-list.sh`, which compares the README and website
  lists with the table, keep the published lists in step. No record states the size of the set.

### Enabling

* A plugin is enabled for a workspace by a dependency (by name or prefix), its own package name, a file
  (`Enabler::File`), a script text, a `package.json` key (`Enabler::ManifestKey`, nested keys with `.`), or in every
  workspace (`Enabler::Always`).
* `[js.plugins] <id> = true | false | { … }` forces a plugin on or off or overrides its patterns, at the top level, in
  `[js.workspaces]` entries and in overrides ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)).
* There are no user-defined plugin rows; a declared tool (`[[js.tools]]`) is not decided.

### Reading configuration

* Configs are read statically only, through the evaluator of `skif-js-syntax`. A plugin with `reads_config` false
  reads the source rather than the exported value and is also called, with a null value, for a config whose export is
  not understood.
* Shared helpers in `skif-js-syntax` read what knip reads from a loaded config: `extract_imported_calls` (calls of
  functions imported from given modules, with evaluated arguments), `extract_property_values` (every value of a key in
  every object literal), `extract_property_calls` (a function stored under a key, called with a given argument),
  `extract_method_calls` (every call of a method name) and `extract_command_calls` (process-spawning calls).
* A config path a script passes is a file whenever it exists, whatever its extension; only a relative path without
  `..` is probed, so nothing outside the package is read.
* A command a plugin finds in a source file or a config is a script for the script parser
  ([ADR 0017](0017-scripts-binaries-and-working-directories.md)). The binaries of a tool whose commands run machine
  tools (`optional_binaries`) are credits, never unlisted.
* A plugin may name a config file that another plugin reads with its own resolver (`LinkedConfig`), and a specifier
  pattern its tool resolves at build time (`VirtualModule`), which is never reported as unlisted, unused or
  unresolved.
* What a build step may write, and which package a shared config module charges, are decided in
  [ADR 0016](0016-config-evaluation-and-incomplete-entries.md) and
  [ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md).

### Growing the set

* A plugin is added when it is used in the corpus or fixes a reported false positive, and it ships with a test that
  fails without it. Its id is knip's id for the same tool.
* A tool without a knip counterpart may have a row when its behaviour is documented by the tool itself; the `knip`
  plugin, which makes knip's own config files entries for a project running both tools, is one.
* A row whose knip counterpart contributes nothing but an enabler is not added.

### Deviations from knip

A plugin follows knip's plugin of the same id unless a deviation meets these rules:

* Where knip loads, calls or compiles something, skif reads the static form that covers the common configs and
  approximates the rest, and an unreadable part makes the entries incomplete rather than guessed.
* An approximation errs towards more entries and more credited dependencies: a plugin keeps its conventional entry
  globs when a config names others, where knip replaces them, and a package loaded on demand is an optional dependency
  that counts as used and is never reported missing. Such a superset may hide an unused file but never reports a used
  one. Jest's `testRegex` follows this rule: it adds entries and keeps the default test globs.
* Where knip's reading would change a command's meaning (dropping one word of a literal argument), skif skips the
  command instead.
* Where knip reports something a project loads (a config file, a stylesheet a component names), skif may credit it.
* The deviation is stated in the plugin module's documentation and pinned by a test in
  `crates/skif-js-plugins/tests/plugins.rs`. The comparison harness counts what remains
  ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)).

### Consequences

* Good, because most plugins are data, cost nothing unless enabled, and are checked by one test each.
* Good, because the shared helpers and the input vocabulary keep config-driven plugins short.
* Good, because a missing plugin's effect can be patched with `entry`, `ignore-dependencies` and `ignore-binaries` until
  the plugin lands.
* Bad, because plugins whose knowledge exists only in the evaluated config degrade to their static reading.
* Bad, because every plugin is knowledge that follows its tool's releases; the knip commit per row makes drift visible
  but does not prevent it.
* Bad, because a superset of entries can hide an unused file.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs`: `ADR_IDS` against the registry, the per-plugin tests such as
  `docusaurus_declares_its_virtual_modules_and_reads_the_babel_config`,
  `a_linked_config_is_read_once_even_when_two_plugins_link_it`, `openclaw_is_enabled_by_its_manifest_key`,
  `machine_tool_commands_judge_only_binaries_reached_through_a_package_manager`,
  `knip_config_files_are_entries_when_knip_is_a_dependency` and `knip_config_in_package_json_is_a_config`.
* `crates/skif-js/tests/graph_fixes.rs`: `jest_test_regex_adds_test_files_and_test_path_ignore_patterns_remove_none`.
* `scripts/check-plugin-list.sh` passes.

## Pros and Cons of the Options

### A declarative table behind one trait, with optional static resolvers and shared extraction helpers

* Good, because plugins are small, reviewable and safe on untrusted input.
* Bad, because static reading cannot follow every config form.

### One hand-written resolver per plugin

* Good, because each plugin is free to read its config however it needs.
* Bad, because the same extraction is written many times, and a plugin that only declares patterns still needs code.

### User-defined plugins in configuration

* Good, because a project could teach skif a tool without a release.
* Bad, because it needs a configuration model for plugins and a way to keep user rows in step with skif's.

### Execute tool configs as knip does

* Good, because every computed config would be read exactly.
* Bad, because it needs a JavaScript runtime and runs untrusted code, which
  [ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md) rules out.
