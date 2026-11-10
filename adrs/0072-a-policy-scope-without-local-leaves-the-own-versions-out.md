---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0030"]
---

# A Policy Scope Without `local` Leaves the Own Versions Out

## Context and Problem Statement

[ADR 0030](0030-harness-known-deviations-and-comparison-scope.md) names skif's default dependency scope: production
and development dependencies, and "a package's own version always joins the policies". That record was written before
[ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md) made `local` a token that
`dependency-types` can name, in `[policy]` as in the groups, so that `["local"]` selects the own versions and
`["!local"]` excludes them. The policy scope never honoured the token: the own version stayed in the catch-all group
whatever `[policy] dependency-types` said, so `["prod", "dev"]` and `["!local"]` still compared every dependent with
the `version` of the workspace package it names and reported `DiffersToLocal`.

That breaks the migration contract of [ADR 0041](0041-skif-migrate-composable-steps-full-validation-and-in-place-merge.md),
that a project the old tools passed passes the migrated configuration. `syncpack lint --dependency-types prod,dev`
leaves the `local` instance out and compares the dependents with each other only. sherif's
`multiple-dependency-versions` compares `dependencies` and `devDependencies` and never a workspace package's own
version. Both migrations reported a dependent at `1.0.0` of a workspace package at `1.0.1`, which neither tool did.

Does a `[policy] dependency-types` list decide whether the own versions take part?

## Decision Drivers

* A token the configuration accepts must do what the reference says it does
* A project that passed sherif or syncpack must pass the migrated configuration
* The default scope (no list) must not change

## Considered Options

* The own version follows `[policy] dependency-types` like every other type
* Keep the own version always in scope and let the migration silence `DiffersToLocal` with an ignored group
* Keep the own version in scope unless the list negates `local`

## Decision Outcome

Chosen option: "The own version follows `[policy] dependency-types` like every other type", because it is what the
token means in a group already, matches syncpack's `--dependency-types`, and leaves the default scope as it is.

**Policy scope.** Without `[policy] dependency-types`, or with an empty list, the own versions take part as before. A
list selects the own versions when `local` passes it as any other type does: a list of plain types must name `local`,
a list of negations only must not negate it. `["prod", "dev"]` and `["!local"]` leave them out; `["prod", "dev",
"local"]` and `["!peer"]` keep them. A dependent of a workspace package outside the scope is then aligned with the
other dependents only, as in syncpack. `--dependency-types` on the command line replaces the list and follows the same
rule.

**Migration.** `skif migrate` writes `[policy] dependency-types = ["prod", "dev"]` for sherif, the sections
`multiple-dependency-versions` compares. syncpack's scope is written as before; its default includes `local`. When both
tools are migrated, the list is the union of the two scopes.

**Comparison harness.** `skif-bench syncpack-diff` passes a fixture's `[policy] dependency-types` to syncpack as it is
and adds `local` only to skif's default scope, where ADR 0030 added it to every list.

### Consequences

* Good, because `local` in `[policy] dependency-types` means what it means in a group.
* Good, because migrated sherif and narrowed syncpack configurations no longer fail projects the tools passed.
* Bad, because a configuration with a positive list that relied on the own-version comparison (`["prod", "dev",
  "peer"]`) loses it until it adds `local`.

### Confirmation

* `skif-rules` unit test `a_policy_scope_that_does_not_select_local_leaves_the_own_versions_out` and the policy
  fixture `dependency-types/policy-scope-without-local`.
* `skif-migrate` golden tests `a_bare_sherif_run_writes_only_the_scope_sherif_compares` and
  `sherif_leaves_the_own_versions_out_of_the_scope_and_syncpack_brings_them_back`; the CLI test
  `a_sherif_migration_does_not_compare_dependents_with_the_version_of_a_workspace_package` lints a stale workspace
  version with and without the migrated configuration.
* `skif-bench` unit test `syncpack_is_asked_for_the_local_instance_when_the_scope_selects_it`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### The own version follows `[policy] dependency-types` like every other type

* Good, because one rule decides every type, the own version included.
* Bad, because a positive list without `local` now changes what is reported.

### Keep the own version always in scope and let the migration silence `DiffersToLocal` with an ignored group

* Good, because no existing configuration changes.
* Bad, because `local` in `[policy] dependency-types` keeps doing nothing, and an ignored `$LOCAL` group also takes the
  dependents of workspace packages out of the semver groups.

### Keep the own version in scope unless the list negates `local`

* Good, because `["!local"]` works and positive lists keep the comparison.
* Bad, because `local` would be the only type a positive list keeps without naming it, and the syncpack migration with
  `--dependency-types prod,dev` would still report what syncpack did not.

## More Information

* Amends: [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md) (the own version joins the policies when
  the scope selects it; the harness adds `local` to the default scope only)
* Related decisions: [ADR 0009](0009-version-and-catalog-policies.md),
  [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md),
  [ADR 0041](0041-skif-migrate-composable-steps-full-validation-and-in-place-merge.md),
  [ADR 0045](0045-migrate-tool-rule-tables-warn-for-config-hints-and-syncpack-override-reading.md)
* See the [ADR index](index.md).
