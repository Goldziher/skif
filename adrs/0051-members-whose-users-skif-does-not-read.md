---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Enum and Namespace Members Whose Users skif Does Not Read

## Context and Problem Statement

[ADR 0040](0040-more-known-deviations-of-the-knip-comparison.md) extends `non-member-project` and `skipped-directory`
of the knip comparison ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)) to skif's side: an export, type or
duplicate only skif reports in a file of knip's workspaces is a known deviation when a file below a project that is not
one of knip's workspaces, or below a directory skif's file discovery skips, imports it. skif's analysis of the
workspace does not read that importer, and knip does. ADR 0040 leaves enum and namespace members with such users
counted as differences, although they follow from the same cause: an importer there that reads `Phase.Start`, reads
`api.Phase.Start` through a namespace import, or passes `Phase` around makes the member used for knip and leaves it
unused for skif. Does the harness set these members apart too, and how does it tell that the importer uses the member?

## Decision Drivers

* A deviation is decided by a test the harness runs on data it reads, never by listing findings (ADR 0040)
* A member counts as used as skif counts it: read by name, or through a whole use of its enum or namespace
  ([ADR 0018](0018-export-namespace-and-member-rules.md))
* The member is credited only through the importers that `non-member-project` and `skipped-directory` already scan for
  exports, so the two sides of a deviation stay one rule

## Considered Options

* Credit a member from the member reads and whole uses the importers' parse records, followed through re-exports
* Credit every member of an enum or namespace that such an importer imports
* Keep the members counted as differences

## Decision Outcome

Chosen option: "Credit a member from the member reads and whole uses the importers' parse records, followed through
re-exports", because skif's parser records both for every imported binding, and an import that reads one member says
nothing of the others.

* An enum or namespace member only skif reports, in a file outside a non-member project and a skipped directory, is
  `non-member-project` when a file below a project that is not one of knip's workspaces uses it, and
  `skipped-directory` when a file below a directory skif's discovery skips uses it.
* A file uses a member when it reads it through a named or default import that does not escape (`Phase.Start`),
  through a member of a namespace import (`api.Phase.Start`), or uses its enum or namespace as a whole as skif counts
  whole uses (an import that escapes, `typeof ns.Phase`, a namespace used as a whole, `import()` and `require`).
* The use follows the `export … from` and `export *` re-exports of the files in between, with the member kept: a read
  of `T.idle` through `export { Tools as T } from './tools'` uses `idle` of `Tools` in `tools`.
* The finding key names the member without its enum (`<file>|<member>`, as knip names it), so the harness reads the
  enum and namespace exports of the finding's file to find the export that has the member.

### Consequences

* Good, because the members of the skif-side mirror follow the same rule as its exports, and the differences left are
  the ones skif's own members need a fix for.
* Bad, because the harness parses the files below non-member projects and skipped directories once more when skif
  reports a member that knip does not.
* Bad, because two enums of one file with a member of the same name share the credit, as their finding keys are the
  same.

### Confirmation

* `tools/bench/src/parity.rs`: the unit test
  `members_only_skif_reports_whose_users_skif_does_not_read_split_off_on_skif_side` pins member reads through a named
  and a namespace import, a whole use, a renamed re-export and the members no importer uses.

## Pros and Cons of the Options

### Credit a member from the member reads and whole uses

* Good, because it credits what skif's own analysis would credit had it read the importer.
* Bad, because it repeats the member part of skif's usage model in the harness.

### Credit every member of an imported enum or namespace

* Good, because it needs only the import scan the exports already use.
* Bad, because it hides the members an importer never reads, which skif rightly reports.

### Keep the members counted as differences

* Good, because nothing new can drift.
* Bad, because the differences that need a fix stay mixed with members whose cause is known.
