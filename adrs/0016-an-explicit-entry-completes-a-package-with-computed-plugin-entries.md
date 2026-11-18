---
status: accepted
date: 2026-11-18
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0008"]
---

# An Explicit Entry Completes a Package with Computed Plugin Entries

## Context and Problem Statement

[ADR 0008](0008-plugins-config-evaluation-and-commands.md) makes a package's entries incomplete when a plugin config
computes its entry keys, and no file or dependency of an incomplete package is judged. A package whose `entry` lists
the entry points itself, as the way out of that state, stayed incomplete: skif reported nothing for it even after the
user had named every entry.

Does a user-written `entry` complete a package whose only incompleteness is computed plugin entries?

## Decision Drivers

* A user who lists the entries has answered the question the incompleteness stands for
* An unreadable config of another kind (unknown value, spread, parse error) can still hide entries
* `plugins.<tool> = false` stays the way to ignore a plugin config altogether

## Considered Options

* Keep the package incomplete whatever `entry` says
* An `entry` that applies in the run mode completes a package whose reasons are all `computed-entries`

## Decision Outcome

Chosen option: "An `entry` that applies in the run mode completes a package whose reasons are all
`computed-entries`", because the explicit list replaces what the computed keys would have named, while any other cause
keeps the package incomplete. In production mode only a pattern ending in `!` applies; a plain pattern is an exclusion
there. Negations and empty or whitespace-only patterns never apply. The list must also match at least one file the
package owns: a stale path (`entry = ["src/old.ts"]`) says nothing about the entries the config computes, so the
package stays incomplete and the end-of-run note still shows.

### Consequences

* Good, because a package with a function-valued bundler config is judged once its entries are written down
* Bad, because entries the computed keys would add beyond the list are reported as unused files
* Bad, because clearing the incompleteness makes every consumer of it apply: `unused-dependencies`,
  `unused-dev-dependencies`, the `unused-subpath-*` rules, the `ignore-files` judging and `stale-js-config`. A
  dependency used only by an entry the computed keys would add becomes an unused-dependency finding

### Confirmation

`crates/skif-js/tests/explicit_entry_complete.rs` covers production and normal runs, a plain pattern in production,
a package without `entry`, stale, empty and negated lists, mixed causes, a workspace table against the top level
and the unused dependency reported after clearing.

## Pros and Cons of the Options

### Keep the package incomplete whatever `entry` says

* Good, because it never judges a file a computed key might add
* Bad, because the only remedy is to turn the plugin off, losing its other findings

### An `entry` that applies in the run mode completes the package

* Good, because it matches what the user stated
* Bad, because a stale list yields false unused files, which `entry` is then the place to fix
