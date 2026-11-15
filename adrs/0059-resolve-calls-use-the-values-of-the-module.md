---
status: accepted
superseded-in-part-by: ["0061"]
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Resolve Calls Use the Values of the Module They Name

## Context and Problem Statement

`require.resolve(specifier)`, `createRequire(…).resolve(specifier)` and `import.meta.resolve(specifier)` return the
path of a module. skif records them as their own import kind, which keeps a local file in use and uses the package it
names, and until now used no export of the module, so `unused-exports` judged the value exports of a file that only a
resolve call names.

Code resolves a module's path to hand it to what loads the module by it, and that loader reads the module's exports:

* a worker pool runs the methods a worker module exports (`new Worker(require.resolve('./worker'), { exposedMethods:
  ['run'] })`, `new Worker(require.resolve('./worker'))`);
* a bundler calls the default export of a loader it is given by path (`{ loader: require.resolve('./css-loader') }`);
* a plugin host calls the plugin a config names by path (`plugins: [require.resolve('./plugins/docs')]`);
* a runtime loads an internal module by its resolved path and reads named exports through its type
  (`requireInternalModule<typeof import('./init')>(require.resolve('./init'))`).

None of these imports an export by name, so every export of such a module was reported though the program calls it.
skif already counts a file named by `new URL('./x', import.meta.url)` or `new Worker(path.join(__dirname, 'x.js'))` as
loaded as a program of its own, every export used; a worker named by `require.resolve` was judged the opposite way.
knip makes a file a resolve call names an entry, so it reports none of its exports or types.

Which exports of a module does a resolve call that names it use?

## Decision Drivers

* A finding must hold for the program as it runs: an export that the module's loader calls is used
* The same load is judged the same way however the code spells the path
* Types are read by name only; a loader reads values, so types used by no importer stay reported

## Considered Options

* Use the values of the module, as a `require()` that binds nothing does
* Use every export of the module, values and types, as for an entry
* Use no export (the previous behaviour)

## Decision Outcome

Chosen option: "Use the values of the module, as a `require()` that binds nothing does", because what loads a module
by its path receives its module object, which holds the values and none of the types.

A resolve call keeps its other properties: it binds nothing, so it is no source of a re-exported binding and gives a
class no known base; it loads nothing at the call site, so it closes no import cycle; a package it names is a used
dependency. `explain-usage` names it `resolve` and says that the module is loaded by its path and every value export
counts. A module that only a plugin input resolves (an extensionless `mise` file task) is a build source as before, so
its exports are still judged.

### Consequences

* Good, because the exports of workers, loaders and plugins named by a resolved path are no longer reported.
* Good, because a resolve call and a `require()` without a binding of the same module now agree.
* Bad, because a resolve call whose path is only printed or compared keeps every value export of the module used.
* Neutral, because the type exports of such a module are still reported unless a used export's signature mentions
  them, which knip, taking the file for an entry, does not report.

### Confirmation

* `crates/skif-js/tests/resolve_calls.rs`: `esm_resolve_calls_keep_the_files_used_and_use_their_values`,
  `a_commonjs_resolve_call_keeps_the_file_used_and_uses_its_values`,
  `the_exports_of_a_module_loaded_by_its_resolved_path_are_used`, `a_type_export_of_a_resolved_module_is_reported`,
  `a_resolve_call_uses_the_values_a_namespace_of_the_module_does_not_read`,
  `explain_usage_names_a_resolve_call_as_using_the_values_of_the_module` and
  `a_module_a_mise_file_task_resolves_is_used_and_its_exports_are_judged`.

## Pros and Cons of the Options

### Use the values of the module, as a `require()` that binds nothing does

* Good, because it matches what the loader receives.
* Bad, because a resolve call that loads nothing credits values too.

### Use every export of the module, values and types, as for an entry

* Good, because it matches knip.
* Bad, because the types of a module loaded by path, which no loader reads, would never be reported.

### Use no export (the previous behaviour)

* Good, because a path that is only printed credits nothing.
* Bad, because every worker, loader and plugin module named by a resolved path reports exports the program calls.
