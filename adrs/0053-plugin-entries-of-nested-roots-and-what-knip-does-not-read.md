---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Plugin Entries of Nested Roots, Published File Loads, and What knip Does Not Read

## Context and Problem Statement

[ADR 0040](0040-more-known-deviations-of-the-knip-comparison.md) sets apart, among others, the exports of a nested
root's default entries as `non-member-project`, and the sources a published `node` `bin` stub loads as
`published-source`, in the knip comparison ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)). Four kinds of
difference remain that follow from how knip reads a project, not from a skif defect:

* knip runs no plugins in a project that is not one of its workspaces, so the files a plugin makes entries there (a
  template's tool and workflow files, a code generator's config and output) report unused exports for knip only.
* A file a manifest publishes as it is, such as `client.js` and `client.d.ts` listed in `files`, loads its source by a
  relative specifier into build output (`require('./dist/client/index')`). skif resolves that specifier to the source;
  knip reports the source as an unused file.
* knip joins a nested `.gitignore` pattern with its directory as a path (`./storage/` in `packages/core/.gitignore`
  becomes `packages/core/**/storage`), while git matches a `./` pattern against nothing. knip then ignores files git
  tracks, and skif's findings in them are skif's only.
* knip treats `export const a = importedA` as a re-export and records no alias for an `export const b = a` next to it,
  so the duplicate `a`/`b` is skif's only.

Does the harness set these apart, and how does it decide each one from data it reads?

## Decision Drivers

* A deviation is decided by a test the harness runs on data it reads, never by listing findings (ADR 0040)
* A plugin entry is decided as skif decides it, so the harness runs skif's plugin resolver, not a copy of it
* A difference that skif's analysis should close stays counted

## Considered Options

* Extend `non-member-project` and `published-source`, and add `knip-dot-ignore` and `re-export-alias` on skif's side
* Keep the four kinds counted as differences

## Decision Outcome

Chosen option: "Extend `non-member-project` and `published-source`, and add `knip-dot-ignore` and `re-export-alias` on
skif's side", because each follows from a rule of knip's that the harness can test.

* `non-member-project` also covers the files skif's plugin resolver makes entries of a nested root that is not one of
  knip's workspaces, and the code config files it reads there. The harness builds the resolver's context from the
  root's manifest (its dependencies and scripts) and reports these files' exports as it reports the root's default
  entries.
* `published-source` also covers the sources that a JavaScript or declaration file a manifest publishes as it is
  (a published path, or a `files` entry without a wildcard) loads by a relative specifier into build output. A loader
  that is itself in build output adds nothing.
* `knip-dot-ignore`: a finding only skif reports, other than a manifest finding, in a file below the directory of a
  nested `.gitignore` whose `./` pattern without wildcards names a directory segment of the file's path below it.
* `re-export-alias`: a duplicate only skif reports whose names hold no `default`, where every alias edge in the group
  targets an export that is itself an alias of an imported binding, and there is at least one such edge.

### Consequences

* Good, because the differences left in these projects are the ones skif needs a fix for.
* Bad, because the harness runs skif's plugin resolver once per nested root that is not one of knip's workspaces.
* Bad, because a plugin entry that knip computes by executing a config (a list of files built at load time) stays a
  difference, as skif reads configs statically.

### Confirmation

* `tools/bench/src/parity.rs`: the unit tests
  `exports_of_the_files_skif_s_plugins_make_entries_of_a_nested_root_split_off`,
  `the_sources_a_published_file_loads_from_build_output_are_published_sources`,
  `findings_in_files_a_dot_slash_gitignore_pattern_hides_from_knip_split_off_on_skif_side` and
  `duplicates_whose_aliases_name_an_export_of_an_import_split_off_on_skif_side` pin the four rules and their negative
  cases.

## Pros and Cons of the Options

### Extend the two deviations and add two on skif's side

* Good, because each rule is a test on files and manifests the harness reads.
* Bad, because `knip-dot-ignore` and `re-export-alias` mirror knip behaviour that may change in a later knip.

### Keep the four kinds counted as differences

* Good, because nothing new can drift.
* Bad, because the count overstates what skif must fix.
