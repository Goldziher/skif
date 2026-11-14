---
status: accepted
date: 2026-11-13
decision-makers: ["Na'aman Hirschfeld"]
---

# Build Sources Handed to a Nested Root

## Context and Problem Statement

A plugin can name a file as an entry (a workflow step's `node build/report.ts`) or as a build source (a Bazel `srcs`
of a root `BUILD` file). Within one root the two differ: an entry's exports are public, a build source is used and
its imports are followed but its exports are judged ([ADR 0031](0031-bazel.md)). A file a plugin names in a nested
root of a multi-root run is listed in `Report::other_project_files` and handed to that root as an extra entry, whichever
kind the plugin named ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)). So an unused export of a
file the outer `BUILD` file builds from is reported when the file's directory is a member of the outer workspace, and
not when it is a root of its own. How does an enclosing root hand a nested root the build sources it names?

## Decision Drivers

* A file a plugin builds from is judged the same in a member and in a nested root
* A file that is both an entry and a build source stays an entry, as within one root
* A host that lints roots itself can tell the two kinds apart without re-reading the plugins
* The lint after `--fix` judges each root as the first lint did

## Considered Options

* A second list, `Report::other_project_build_sources`, handed to the nested root with
  `RunOptions::with_extra_build_sources`
* One list of files of other projects, each with its kind
* Hand every file of a nested root a plugin names as an entry, as before

## Decision Outcome

Chosen option: "A second list, `Report::other_project_build_sources`, handed to the nested root with
`RunOptions::with_extra_build_sources`", because it adds the build sources beside the entries a host already routes,
with the same routing, and leaves `Report::other_project_files` with one meaning.

### The two lists

* `Report::other_project_files` lists the files of other projects that the analysed code imports, that a script runs
  or that a plugin names as an entry, and every module the graph reached outside the root.
* `Report::other_project_build_sources` lists the files of other projects that a plugin names as build sources. Both
  are absolute, sorted and not rendered. A file can be in both lists.
* This replaces "a file a plugin names there" in the crediting across roots of
  [ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md), which now reads "a file a plugin names there as
  an entry", and "where it is an entry" for a build source that names a file of a nested root in
  [ADR 0031](0031-bazel.md), which now reads "where it is a build source".

### Routing

* A file of either list below the listing root goes to the innermost root of the run that holds it, made relative to
  that root, also when other roots lie in between: a file of `mid/inner` that the outer root names goes to
  `mid/inner`, not to `mid`. `skif -r` passes the files of the first list to `RunOptions::with_extra_entries` of that
  root's run and those of the second to `RunOptions::with_extra_build_sources`.
* A file of either list outside the listing root is credited as used by the root that holds it, and that root drops
  its `unused-files` finding, as before.
* The nested root makes a handed build source it owns a build source: used, its imports followed, its exports judged.
  A file handed both as an entry and as a build source is an entry.
* A package of the nested root that owns a handed entry counts as imported by another root, so
  `unused-subpath-exports` does not judge its `exports`; a handed build source does not make it so.
* The lint after `--fix` hands each root the same entries and build sources again.

### Consequences

* Good, because an unused export of a file the outer `BUILD` file builds from is reported whether its directory is a
  member or a nested root.
* Good, because `other_project_files` keeps one meaning: the files the receiving root makes entries.
* Bad, because a host that lints nested roots itself must read and hand on a second list, or the build sources of
  its nested roots are reported unused.
* Bad, because a root linted alone still cannot see what an enclosing root builds from, and reports such a file
  unused.

### Confirmation

* `crates/skif-js/tests/fp_resolve.rs`: `a_root_bazel_source_in_a_nested_root_is_a_build_source_of_another_project`,
  `a_build_source_from_another_root_is_used_and_its_exports_are_judged` and
  `a_file_handed_as_an_entry_and_a_build_source_is_an_entry`.
* `crates/skif-cli/tests/multi.rs`: `a_bazel_source_of_a_nested_root_is_used_there`,
  `fixing_keeps_the_bazel_sources_of_a_nested_root` and
  `a_bazel_source_two_roots_down_is_a_build_source_of_the_innermost_root`.
* `crates/skif-cli/src/roots.rs`: `route_hands_build_sources_below_to_the_innermost_root_and_credits_those_outside`.

## Pros and Cons of the Options

### A second list, `Report::other_project_build_sources`, handed to the nested root with `RunOptions::with_extra_build_sources`

* Good, because the routing of the first list serves the second unchanged.
* Good, because each list says what the receiving root makes of its files.
* Bad, because a file named both ways appears in both lists.

### One list of files of other projects, each with its kind

* Good, because a file appears once.
* Bad, because every host that reads `other_project_files` changes, also those that only hand on entries.

### Hand every file of a nested root a plugin names as an entry, as before

* Good, because nothing changes for hosts.
* Bad, because the exports of a file a `BUILD` file builds from are public in a nested root and judged in a member.
