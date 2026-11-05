---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Version Groups Own Their Instances: The Catalog Rules Leave Claimed Instances Alone

## Context and Problem Statement

[ADR 0009](0009-version-and-catalog-policies.md) makes the catalog a global `[catalog] mode` next to the version groups, and says only that a dependency a catalog owns is not also reported as a version mismatch. It does not say what happens when a configured version group matches an instance of such a dependency. The catalog rules ignored version groups entirely: an `ignored` group did not stop `catalog-not-used`, a `pinned` group and `catalog-not-used` proposed two different fixes for one span, and a `banned` dependency with a catalog entry was reported as banned and as not using the catalog. In syncpack there is no such conflict, because the catalog is a version group like any other and the first group that matches an instance owns it.

## Decision Drivers

* No instance may be reported, or fixed, by two rules with different fixes
* A monorepo must be able to keep one package off the catalog (a pinned "baseline") while the rest use `catalog:`
* Follow syncpack's first-match-wins order
* The behaviour must be testable with both families of rules running together

## Considered Options

* A configured version group that matches an instance claims it, whatever its policy, and the catalog rules skip it
* Only `ignored` groups exempt an instance from the catalog rules
* Keep the catalog rules independent and let users configure `[rules]` to avoid the overlap

## Decision Outcome

Chosen option: "A configured version group that matches an instance claims it, whatever its policy, and the catalog rules skip it", because it is the syncpack order, resolves every overlap the same way and needs no per-policy cases.

**Claiming.** An instance is claimed when the first configured `[[version-groups]]` entry whose selectors match it exists. The policy of that group (`pinned`, `banned`, `ignored`, `snapped-to`, `same-range`, `same-minor`, a preferred semver group with an explicit selector) decides what is reported; the catalog is the catch-all behind the configured groups. Semver groups only choose a range and claim nothing.

**What the catalog rules skip.** `catalog-not-used`, `catalog-mismatch` and `catalog-missing-entry` do not see a claimed instance: it is neither a candidate nor a sibling, does not count toward `min-packages` and does not decide the version of an entry created for a missing dependency. `unresolved-catalog-references` does not report a claimed instance, in a dependency section or in an override section. `unused-catalog-entries` still counts a claimed `catalog:` reference as a use, because it judges the entry and removing it would break the reference.

**Defaults that follow.** `catalog-mismatch` only reports under `mode = "off"`, so its default severity is a warning. `unresolved-catalog-references` also reads the override sections, through the same enumeration as `unused-catalog-entries`.

### Consequences

* Good, because `ignored`, `pinned` and `banned` groups behave the same with and without a catalog.
* Good, because a baseline group keeps one package off the catalog without silencing the catalog rules for the others.
* Bad, because a group without a `dependencies` selector (for example `packages = ["legacy"]`) exempts everything in that package from the catalog rules; the group has to name what it covers.
* Bad, because an `unresolved` reference in a claimed instance is no longer reported by the catalog rules; the group owns the judgment.

### Confirmation

* The fixture group `catalog-groups` runs the catalog and the policy rules together and plans their fixes jointly, so an instance reported or fixed twice fails the case.
* Unit tests in `policy/mod.rs` run every policy and catalog rule over one workspace for each policy and assert one report per instance.

## Pros and Cons of the Options

### A configured version group that matches an instance claims it, whatever its policy, and the catalog rules skip it

* Good, because it is one rule with no per-policy cases.
* Good, because it matches syncpack.
* Bad, because a broad selector hides the instances from the catalog rules.

### Only `ignored` groups exempt an instance from the catalog rules

* Good, because the change is small.
* Bad, because `pinned` and `banned` still produce two reports and two fixes for one span.

### Keep the catalog rules independent and let users configure `[rules]` to avoid the overlap

* Good, because nothing changes in the rules.
* Bad, because the overlap is a defect in the defaults, not a configuration choice.

## More Information

* Refines [ADR 0009](0009-version-and-catalog-policies.md) where it leaves the interplay of groups and the catalog open.
* Related: [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md)
