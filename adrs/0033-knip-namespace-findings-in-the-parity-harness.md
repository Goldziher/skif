---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# knip Namespace Findings in the Parity Harness: Where knip Reports What `unused-namespace-exports` and `unused-namespace-types` Find

## Context and Problem Statement

[ADR 0010](0010-js-analysis-on-oxc.md) maps the knip issue types `nsExports` and `nsTypes` to `unused-namespace-exports` and `unused-namespace-types`, and [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md) restores both rules on the grounds that the two issue types "have a precise counterpart again". The parity harness grouped `nsExports` and `nsTypes` with `namespaceMembers` and knew neither rule, so the findings of namespace-only modules left the `exports` and `types` comparison on the skif side only.

The knip 6.44.1 source and its JSON on a fixture show that the mapping is not by issue type. `nsExports` and `nsTypes` are off by default (`defaultExcludedIssueTypes`) and cover a module whose namespace import is used as a whole value. A module whose namespace import is read member by member (`ns.used`) reports its unreferenced exports and types under `exports` and `types`, each item carrying the namespace in a `namespace` field (`parentSymbol`). That second case is what `unused-namespace-exports` and `unused-namespace-types` report, and a whole-namespace use never produces a skif finding (ADR 0029).

## Decision Drivers

* A rule and the knip findings it corresponds to must be compared under one category
* The comparison must not depend on an issue type knip leaves off by default
* The mapping must come from knip's behaviour, not from its issue-type names
* Locked records stay unedited

## Considered Options

* Compare `unused-namespace-exports` and `unused-namespace-types` with the `exports` and `types` items that carry a `namespace`, and with `nsExports` and `nsTypes` when present
* Map the rules to `nsExports` and `nsTypes` only
* Leave the two rules out of the comparison

## Decision Outcome

Chosen option: "Compare `unused-namespace-exports` and `unused-namespace-types` with the `exports` and `types` items that carry a `namespace`, and with `nsExports` and `nsTypes` when present", because it is the only mapping under which the default knip run and skif see the same findings.

The harness has the categories `nsExports` and `nsTypes`. On the skif side they hold the findings of `unused-namespace-exports` and `unused-namespace-types`, which no longer count in `exports` and `types`. On the knip side they hold the items of `nsExports` and `nsTypes` and the items of `exports` and `types` that have a `namespace`. `namespaceMembers` holds only the knip issue type of that name and `unused-namespace-members`. A knip report where a module's namespace is used as a whole reports nothing skif would, which shows as knip-only findings when `nsExports` is included in the knip run.

The committed `parity.json` baseline is regenerated after this change, because its counts predate the key and category changes of the harness (member keys without the qualifier, ADR 0030, and the namespace categories).

### Consequences

* Good, because the findings of the two namespace rules are compared with the knip findings they correspond to.
* Good, because the `exports` and `types` categories are comparable again after ADR 0029 moved findings out of them.
* Bad, because the mapping depends on knip reporting a `namespace` field, so a knip upgrade that drops it shows as a baseline drift.

### Confirmation

* The `skif-bench` unit tests parse the JSON knip prints (every issue type present on every row, members as `{ namespace, name }`, namespaced `exports` and `types`) and assert that each member and namespace finding agrees with its skif counterpart.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Compare `unused-namespace-exports` and `unused-namespace-types` with the `exports` and `types` items that carry a `namespace`, and with `nsExports` and `nsTypes` when present

* Good, because it matches what the default knip run reports.
* Bad, because it relies on a knip JSON field.

### Map the rules to `nsExports` and `nsTypes` only

* Good, because it follows the issue-type names in ADR 0010.
* Bad, because those types are off by default and mean a whole-namespace use, so the two sides would never agree.

### Leave the two rules out of the comparison

* Good, because it needs no mapping.
* Bad, because the namespace findings would be invisible to the gate, and `exports` and `types` would stay skewed.

## More Information

* Amends: [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md), [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md)
* See the [ADR index](index.md).
