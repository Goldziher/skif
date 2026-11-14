---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Plugin Entries in Build Output

## Context and Problem Statement

A tool configuration can name its entry as the file a build writes rather than the source it is built from: a webpack
config bundles `path.join(__dirname, 'lib-esm', 'entry.js')`, the output `tsc` writes for `src/entry.tsx`. A path
`package.json` publishes into build output stands for its source, and one with no source behind it makes the package
incomplete as `unmapped-published` ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)). A plugin entry got
neither: one that matched no file was no entry, so every file the bundle reaches was reported by `unused-files`, its
dependencies by `unused-dependencies`, and the package was judged as if its entries were known. How does a plugin entry
that names build output count?

## Decision Drivers

* A file a build compiles to stands for the same source wherever it is named: in `package.json`, in a script or in a
  tool configuration
* An entry the analysis cannot follow never turns what it reaches into unused-file findings, and the package says why
* A path into nothing stays no entry and does not silence the package, as a published path into nothing does
* Plugins name default entries (`remix.init/index.js`) and every branch of a choice whether or not the file exists, so
  a missing file alone does not make a package incomplete

## Considered Options

* Map a plugin entry that is no file as a published path is; one in build output with no source behind it makes the
  package incomplete with a cause of its own
* Map it, and drop one that maps to nothing as before
* Make the package incomplete for every plugin entry that names no file

## Decision Outcome

Chosen option: "Map a plugin entry that is no file as a published path is; one in build output with no source behind
it makes the package incomplete with a cause of its own", because it reads the same build as the manifest does and
leaves no package judged while an entry it cannot follow hides what it reaches.

### Mapping

* A plugin entry that names one path in its package, and that is neither a file nor a directory, stands for the source
  a published path maps to: through the `outDir` and `rootDir` of the package's build directories (`tsconfig`), then
  from a build output directory (`dist`, `lib`, `lib-esm`, ...) to its `src` twin, with the extensions a compiler maps
  (`.js` to `.ts` and `.tsx`, `.mjs` to `.mts`, ...). The source is an entry of the package, as the plugin's own entry
  would be.
* An entry that exists is unchanged, and one outside the package is handed to the root that owns it as before.

### Incomplete entries

* A plugin entry that maps to no source and is build output, judged as for `unmapped-published` (it lies in a build
  output directory, or in `bin` where a build step may write it, and has no extension other than a code one), makes
  its package's entries incomplete for the whole package with the cause `unmapped-entry`. The
  reason's `file` is the entry, relative to the workspace root in a report.
* Each such entry is a reason of its own, unlike `unmapped-published`, which one mapped published path clears: every
  bundle entry is a separate root of the module graph.
* A plugin entry into nothing outside build output is no entry and leaves the package judged, as a published path into
  nothing does.
* This adds `unmapped-entry` to the causes [ADR 0016](0016-config-evaluation-and-incomplete-entries.md) and
  [ADR 0022](0022-reporting-and-output-formats.md) list for `incomplete-packages`.

### Consequences

* Good, because a bundler config that names its output gets the files and dependencies of its source judged as used.
* Good, because a bundle whose source skif cannot find leaves its package unjudged with a reason naming the entry,
  instead of reporting what it reaches as unused.
* Bad, because a command a tool configuration runs (`node dist/cli.js` in a workflow step) whose output has no source
  skif can map now leaves its package unjudged, where a `package.json` script running it does not.
* Bad, because a branch of a choice that names build output without a source makes the package incomplete even when the
  branch taken names source.

### Confirmation

* `crates/skif-js/tests/build_output_entries.rs`: `a_plugin_entry_in_an_out_dir_maps_back_to_its_source` and
  `a_plugin_entry_in_a_build_directory_maps_back_to_src`.
* `crates/skif-js/tests/explain_absence.rs`: `a_plugin_entry_in_build_output_without_a_source` and
  `a_plugin_entry_into_nothing_leaves_the_package_judged`.

## Pros and Cons of the Options

### Map a plugin entry that is no file as a published path is; one in build output with no source behind it makes the package incomplete with a cause of its own

* Good, because the manifest and the plugins read build output alike.
* Good, because the cause names the entry, so the fix (pointing the config at its source, or `[js] entry`) is clear.
* Bad, because entries a command names are judged more strictly than a script's.

### Map it, and drop one that maps to nothing as before

* Good, because no package becomes incomplete that was judged before.
* Bad, because a bundle with an unmapped source still reports everything it reaches as unused.

### Make the package incomplete for every plugin entry that names no file

* Good, because no missing entry is ever ignored.
* Bad, because plugin defaults and the branches of a choice name files that do not exist, so most packages using
  such a tool would go unjudged.
