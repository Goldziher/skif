---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Packages Named on an Environment-Gated Branch

## Context and Problem Statement

The config evaluator keeps every branch of a choice whose test it cannot decide
([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)): a ternary, `&&`, `||`, `??` or `if` on an unknown test
yields what the branches share, a `ConfigValue::OneOf` of their values, or, for a conditional spread, every key a branch
sets. A plugin that derives a package name from such a value emits `PluginInput::Dependency`, a required config
reference that is `unlisted-dependencies` with the detail `ConfigReference` when no manifest lists it
([ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md)). A shared Vitest config that spreads
`coverage: { provider: 'v8' }` only when `process.env.COVERAGE_ENABLED === 'true'` therefore makes
`@vitest/coverage-v8` unlisted in every package that calls its factory, although the provider loads only in the runs
that set the variable, and knip, which runs the config in its own environment, sees no provider at all. Is a package
named only on a branch of a choice the environment makes a required reference?

## Decision Drivers

* A finding is one the package must act on in every run of the tool, not only in the runs a variable selects
* A listed package that a config may load stays used, so `unused-dependencies` does not ask to remove it
* A package a config names whatever the environment holds stays required, so the precision of
  [ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md) for unconditional names is kept
* A choice whose test reads something other than the environment (a file on disk, a helper's result) is no reason to
  relax a reference
* One mechanism for every plugin, not a rule each plugin that derives package names must repeat

## Considered Options

* An optional reference: a package named only on an environment-gated branch credits a listing but never makes a
  package unlisted
* Keep the reference required and name the environment condition in the finding
* Keep every branch required, as before

## Decision Outcome

Chosen option: "An optional reference: a package named only on an environment-gated branch credits a listing but never
makes a package unlisted", because such a package is needed only in the runs that set the variable, which is what an
optional reference means, and because it judges the package the way the tool behaves when the variable is not set.

### Environment-gated choices

* A test reads the environment when it holds a member of `process.env` or `import.meta.env` (`process.env.CI`,
  `process.env['CI']`, `import.meta.env.MODE`), under any operator (`!`, `===`, `!==`, `&&`, `||`, a call), directly or
  through a top-level constant it names (`const coverage = process.env.COVERAGE === 'true'`). A local binding of a
  function and a parameter that receives such a value are not followed.
* When the evaluation leaves such a test undecided, the config is evaluated twice more: once with every such test taken
  as true and once with every such test taken as false, in the config and in every module it imports values from
  (`ExtractedConfig::environment_variants`). A test that does not read the environment stays undecided in both. A
  falsy left side of `a && b` taken this way is `false`, so a spread or `.filter(Boolean)` drops it.
* The plugin reading the config reads each variant as well. A package it requires from the config
  (`PluginInput::Dependency`) that it does not require from both variants becomes a `PluginInput::OptionalDependency`:
  it credits a manifest that lists it, so it is not `unused-dependencies`, and it is never `unlisted-dependencies`.
* A package named on both branches, on every branch, or under a test that does not read the environment keeps the
  judgment of [ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md). This supersedes, for such packages only,
  the rule there that every name a plugin derives from a config value is a required reference.

### Consequences

* Good, because a coverage provider, reporter or environment a shared config enables only in CI is no longer unlisted
  in every package calling it, matching knip on such configs.
* Good, because every plugin that derives package names from config values is covered, with no plugin changed.
* Good, because a config without an undecided test on the environment is evaluated once, as before.
* Bad, because a package a CI run does need and no manifest lists is no longer reported; the run that sets the
  variable reports the missing module itself.
* Bad, because a config with such a test is evaluated three times and read by its plugin three times.
* Bad, because taking every test as true, then as false, enumerates two outcomes, not every combination: a package
  named under both `MODE === 'a'` and `MODE !== 'a'` counts as optional although every run needs it, one named under
  `A ? (B ? x : y) : (B ? y : x)` counts as required although `A` without `B` does not need it, and a test that
  reaches the environment through a local binding or a parameter is not recognised.

### Confirmation

* `crates/skif-js-syntax/tests/config_functions.rs`: `a_test_on_the_environment_gives_a_variant_for_each_outcome` and
  `a_test_that_does_not_read_the_environment_gives_no_variants`.
* `crates/skif-js/tests/config_references.rs`: `a_package_named_only_on_an_environment_gated_branch_is_optional` and
  `a_package_named_on_a_branch_of_another_unknown_condition_stays_required`.

## Pros and Cons of the Options

### An optional reference: a package named only on an environment-gated branch credits a listing but never makes a package unlisted

* Good, because findings follow the run without the variable, which is how knip and a local run see the config.
* Good, because a listing of the package still counts as used.
* Bad, because a package missing only in CI goes unreported.

### Keep the reference required and name the environment condition in the finding

* Good, because a package missing in CI is still reported.
* Bad, because the finding stays in every package calling the factory, and the advice moves the burden of a decision
  the shared config made onto each of them.
* Bad, because each plugin would have to carry the condition into its inputs.

### Keep every branch required, as before

* Good, because nothing changes.
* Bad, because shared configs that turn tools on by environment report the same false finding in every package that
  uses them.
