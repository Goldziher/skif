---
status: accepted
date: 2026-11-02
decision-makers: ["Na'aman Hirschfeld"]
---

# JS Rule Set, Default Severities, Addon Profile and Windows Support

## Context and Problem Statement

[ADR 0010](0010-js-analysis-on-oxc.md) maps fifteen knip issue types to rule ids, but the implementation provides thirteen: `unused-namespace-exports` and `unused-namespace-types` do not exist. It also leaves the default severity of the JS rules open, and a default-on analysis that reports every unused export as an error would fail the first run on most real repositories. [ADR 0018](0018-npm-distribution-via-napi-rs.md) calls for an unwinding build profile for the napi addon without naming it, [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md) lists the workspace crates incompletely, and the statement that Windows has no npm package could be read as dropping Windows altogether. These points need one authoritative statement.

## Decision Drivers

* The rule list in the records must equal the rule list in the registry
* A default-on analysis must not turn uncertain findings into failing builds
* A panic in the napi addon must not end the host Node or Bun process
* Windows users keep every channel that does not depend on npm packages

## Considered Options

* Withdraw the two namespace rules, set tiered default severities, require an unwinding addon profile and restate Windows support
* Implement the two namespace rules before 0.1.0 and keep every JS rule an error
* Leave the records as written

## Decision Outcome

Chosen option: "Withdraw the two namespace rules, set tiered default severities, require an unwinding addon profile and restate Windows support", because it makes the records match the registry, keeps first runs usable and does not add rules whose precision has not been measured.

**Rule set.** The JS rules are the thirteen registered ones: `unused-files`, `unused-dependencies`, `unused-dev-dependencies`, `unused-optional-peer-dependencies`, `unlisted-dependencies`, `unlisted-binaries`, `unresolved-imports`, `unused-exports`, `unused-types`, `unused-enum-members`, `unused-namespace-members`, `duplicate-exports` and `circular-imports`. The mapping of [ADR 0010](0010-js-analysis-on-oxc.md) entries `nsExports` and `nsTypes` to `unused-namespace-exports` and `unused-namespace-types` is withdrawn. Reporting an unused export of a namespace or an unused type inside one needs the namespace's member usage to be tracked per consumer, which `unused-namespace-members` and `unused-exports` do not yet share, and a rule that guesses would produce false positives on a default-on analysis. Neither id is released, so neither is reserved by [ADR 0007](0007-rule-and-diagnostic-model.md); a later record may add them.

**Default severities.** `unused-dependencies`, `unlisted-dependencies` and `unresolved-imports` default to `Error`. `unused-files`, `unused-exports`, `unused-types`, `unlisted-binaries` and `unused-dev-dependencies` default to `Warning`, because their precision depends on entry-point and binary detection that varies most between projects. `unused-optional-peer-dependencies`, `unused-enum-members`, `unused-namespace-members` and `duplicate-exports` default to `Warning`, and `circular-imports` is off by default. `[rules]` and `--rule` raise or lower any of them, and `--fail-on-warnings` turns the warnings into a failing run.

**Addon profile.** The CLI binaries keep `panic = "abort"` in `[profile.release]`. The napi addon is built with a separate Cargo profile that inherits `release` and sets `panic = "unwind"`, selected by the napi build command, so that a panic reaches the napi boundary as a JavaScript exception. Building the addon with the plain release profile is a defect.

**Windows.** Windows remains a supported platform: the GitHub release attaches the `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` archives, `cargo binstall skif-cli` resolves them, and the Scoop manifest installs them. CI runs the test suite on Windows. Only the npm packages omit Windows ([ADR 0018](0018-npm-distribution-via-napi-rs.md)).

**Crate list.** This corrects the crate list of [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md). The workspace contains `skif-core`, `skif-specifier`, `skif-config`, `skif-edit`, `skif-workspace`, `skif-lint`, `skif-rules`, `skif-js-syntax`, `skif-js-plugins`, `skif-js`, `skif-report`, `skif`, `skif-cli` and `skif-node`. `skif-js` exists and is published. `skif-node` is the napi addon, is `publish = false` and reaches users only through the npm packages. `tools/bench` (`skif-bench`) is also `publish = false`.

### Consequences

* Good, because the records and the registry list the same rules.
* Good, because a first run on a real repository fails on dependency problems, not on dead-code heuristics.
* Good, because an addon panic is catchable by the host.
* Bad, because the addon needs its own build profile and the CI jobs that build it must select it.
* Bad, because the warning tier hides real unused exports from a build that does not pass `--fail-on-warnings`.

### Confirmation

* `docs/rules.md` and `rule-ids.txt` list thirteen JS rules and no `unused-namespace-exports` or `unused-namespace-types`.
* `skif rules --format json` reports the default severities above for the JS rules.
* A test that panics inside the addon observes a thrown JavaScript error, not a dead process.
* The release assets include the two Windows archives, and the CI test matrix includes a Windows runner.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Withdraw the two namespace rules, set tiered default severities, require an unwinding addon profile and restate Windows support

* Good, because it matches what exists and what is safe to run by default.
* Bad, because two knip issue types have no skif counterpart in 0.1.0.

### Implement the two namespace rules before 0.1.0 and keep every JS rule an error

* Good, because knip's issue types would be covered completely.
* Bad, because the rules would be new, unmeasured code, and error-level dead-code findings fail first runs.

### Leave the records as written

* Good, because it needs no work.
* Bad, because the records name rules that do not exist and leave the addon's panic behaviour and Windows support ambiguous.

## More Information

* Amends: [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0018](0018-npm-distribution-via-napi-rs.md), [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md)
* Related decisions: [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0013](0013-distribution-and-naming.md), [ADR 0017](0017-release-policy-a-single-complete-0-1-0.md)
* See the [ADR index](index.md).
