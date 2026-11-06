---
status: accepted
date: 2026-11-06
decision-makers: ["Na'aman Hirschfeld"]
---

# Catalog Version Divergence and Reasons on Ignores

## Context and Problem Statement

Two reports against sherif apply to skif ([#1](https://github.com/Goldziher/skif/issues/1), [#4](https://github.com/Goldziher/skif/issues/4)). A `catalog:` reference hides its version from `version-mismatch` ([ADR 0009](0009-version-and-catalog-policies.md)), so `catalog:` at 5.9.3 next to `catalog:ts6` at 6.0.3 reports nothing, and moving to catalogs loses coverage silently. And a large baseline of `[[ignore]]` entries cannot say why each one exists, so a deliberate split looks like leftover debt.

## Decision Drivers

* Moving to catalogs must not silently lose version comparison
* Named catalogs often exist to run two majors side by side, so a difference between them is not always a defect
* A deliberate exception should be recordable in the configuration and reviewable

## Considered Options

* A new warning rule, `catalog-version-divergence`, and a `reason` on `[[ignore]]`
* Resolve `catalog:` references inside `version-mismatch`
* TOML comments only for ignore reasons

## Decision Outcome

Chosen option: "A new warning rule, `catalog-version-divergence`, and a `reason` on `[[ignore]]`", because resolving references inside `version-mismatch` would make an error of a common, deliberate layout and cross the ownership line ADR 0009 draws between the version and catalog rules, and because a comment cannot be enforced.

**`catalog-version-divergence`** reports every reference to a dependency that resolves, through `catalog:` or `catalog:<name>`, to a different version from another reference to it. Its state is `DiffersToOtherCatalog`. It runs in every catalog mode, compares the versions as written, leaves peer dependencies out because they state a compatible range, and leaves an instance a version group claims to that group. It is a warning and has no fix: which catalog is right is the owner's decision.

**`catalog-mismatch` names the version** of the catalog its siblings use, so a literal beside a catalog shows both sides.

**`[[ignore]]` takes an optional `reason`.** A blank reason is a configuration error. `--require-ignore-reasons` fails the run with exit code 2 on an entry from a file without one. Entries from flags have nowhere to carry a reason and are exempt; the `--ignore-dependency` entry records the flag as its reason.

### Consequences

* Good, because a workspace on catalogs keeps version comparison, and a deliberate split is documented where it is declared.
* Good, because `--require-ignore-reasons` lets a team ratchet a baseline down.
* Bad, because a workspace with intentional side-by-side catalogs sees warnings until it adds an `[[ignore]]` entry.
* Bad, because versions are compared as written, so `^5.9.3` and `5.9.3` count as different.

### Confirmation

* `fixtures/policy/catalog/catalogs-resolve-to-different-versions` reproduces the report of #1.
* `require_ignore_reasons_fails_only_on_entries_of_a_file_without_one` in `crates/skif-cli/tests/cli.rs` covers the flag.
