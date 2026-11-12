---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# `skif migrate --merge` Unites List Entries by Name

## Context and Problem Statement

[ADR 0041](0041-skif-migrate-composable-steps-full-validation-and-in-place-merge.md) made `--merge` an in-place edit
of the existing `skif.toml`, in which a list both files set gains the entries it lacks. An entry was compared by its
text, so two forms of one entry both stayed. `ignore-workspaces = ["playground/**"]` merged with knip's
`{ name = "playground/**", reason = "test suites" }` kept the string and the table. In `entry` and `project` the result
also changes what skif checks: `entry = ["src/main.js"]` merged with knip's `src/main.js!` became
`["src/main.js", "src/main.js!"]`. In a production run only the `!` patterns make entries and the other patterns are
excluded from the project, so the file the migrated pattern names was left out of the production run, and every file
it reaches was reported as unused.

How should `--merge` decide that two list entries are the same, and which form should the united entry take?

## Decision Drivers

* Merging a migration into a file must not change what the migrated settings check
* An existing value the user wrote keeps priority where the two values conflict
* Merging the output of a migration into itself changes nothing
* The rule follows what each list means, not the text of its entries

## Considered Options

* Match list entries by name, with a rule per list for the `!` production marker
* Match entries by name only, keeping `x` and `x!` as different entries and reporting the pair
* Keep comparing the text of the entries

## Decision Outcome

Chosen option: "Match list entries by name, with a rule per list for the `!` production marker", because the forms
that differ only in a reason or a marker describe one entry, and for each list the meaning of the marker decides which
form keeps every run unchanged or matches the migrated tool.

**Names.** An entry is a string or a `{ name, reason }` table (`reason` optional); a table with other keys, and any
other value, matches only an entry with the same text. A string and a table with that name are one entry. The united
entry keeps the existing reason; an existing entry without one takes the migrated reason. Two different reasons keep
the existing one, and the report lists the key (`kept-existing`). The entry keeps its place and formatting in the
existing list.

**The production marker.** In the lists of `[js]` and `[js.workspaces."<dir>"]`, `x` and `x!` are one entry:

* `entry` and `project`: the united entry keeps the marker when either file has it. In a normal run `x` and `x!` are
  the same pattern, so a normal run is unchanged. In a production run `x!` makes the entry and `x` excludes the file,
  so keeping both would undo the marker; the marked form is what the file that has it asks for. When the merge adds
  the marker to an existing pattern, the report names the list (`replaced-existing`).
* The ignore lists (`ignore`, `ignore-files`, `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`,
  `ignore-members`, `ignore-workspaces` and their `extend-` keys): the united entry keeps the marker only when both
  have it. `x` applies in every mode and `x!` in production mode only, so the union of the two is `x`.

Other lists keep the marker as part of the entry. A plain pattern that only overlaps a `!` pattern (`src/**` next to
`src/main.js!`) is a different entry and stays; detecting such overlaps needs the files of the project and is left to
the analysis.

### Consequences

* Good, because a migrated production entry keeps its file in the production run after a merge.
* Good, because a merged list holds each entry once, with its reason.
* Good, because the marker rule of each list follows what skif does with the marker in that list.
* Bad, because a merge can now change the text of an existing entry, which the report lists for `entry` and
  `project` only.
* Bad, because a plain pattern that overlaps a merged `!` pattern without being the same glob still excludes the files
  they share from a production run.

### Confirmation

* `crates/skif-migrate/src/merge.rs`: unit tests for the marker in `entry`, `project` and the ignore lists in both
  orders, a plain entry taking a reason, a reason kept over the plain form and over a different reason, and the
  marker staying part of an entry outside the `[js]` lists.
* `crates/skif-migrate/tests/golden.rs`:
  `merging_knip_keeps_its_production_entries_and_reasons_over_the_plain_existing_ones`.
* `crates/skif-cli/tests/migrate.rs`: `merge_keeps_one_entry_per_name_with_the_production_marker_and_the_reason`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Match list entries by name, with a rule per list for the `!` production marker

* Good, because no run changes what the united entries check, apart from the production entry the migration adds.
* Bad, because the merge needs to know which lists take the marker and what it means there.

### Match entries by name only, keeping `x` and `x!` as different entries and reporting the pair

* Good, because the merge needs no knowledge of the lists.
* Bad, because the merged configuration still drops the production entry until the user edits it.

### Keep comparing the text of the entries

* Good, because it needs no code.
* Bad, because a merged knip configuration loses its production entries and duplicates its ignore entries.

## More Information

* Related decisions: [ADR 0034](0034-js-configuration-parity-with-knip.md) (the `!` marker in `entry` and `project`),
  [ADR 0041](0041-skif-migrate-composable-steps-full-validation-and-in-place-merge.md) (the in-place merge),
  [ADR 0074](0074-reasons-on-js-ignore-entries.md) (`{ name, reason }` entries)
* See the [ADR index](index.md).
