---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Third-Party Types in the Facade API

## Context and Problem Statement

[ADR 0003](0003-library-first-embedding.md) says that no `serde_json::Value`, `toml::Value`, `nodejs_semver` or `rayon` type appears in the facade's public signatures, and that a rustdoc-JSON test over `skif` asserts it. Neither holds. The `skif` crate documentation lists `serde_json`, `toml`, `indexmap` and `nodejs_semver` as the contract of the data model, a fix carries a `serde_json::Value`, a rule's options are a `toml::Table`, and the parsed range of a dependency specifier is a `nodejs_semver` type. [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md) withdrew the rustdoc-JSON test, which needs a nightly toolchain, and left the allow-list written in two places that disagree.

## Decision Drivers

* The allow-list of third-party types must be stated once, where a reader of the API looks
* A rule that nothing checks is not a rule
* The check must run on the stable toolchain skif is built with
* A new leak must fail a test, not wait for review

## Considered Options

* State the allow-list once in the `skif` crate documentation, record it here, and audit the public sources in a stable test
* Wait for a rustdoc-JSON test on a nightly toolchain
* Keep ADR 0003 as written and rely on review

## Decision Outcome

Chosen option: "State the allow-list once in the `skif` crate documentation, record it here, and audit the public sources in a stable test", because it makes the rule true today and checkable on the stable toolchain.

**The allow-list.** A public item of `skif`, or of a crate it re-exports from, may name a type from these third-party crates and from nothing else outside skif and the standard library:

| Library | Where it shows |
| --- | --- |
| `serde` | the `Serialize` and `Deserialize` impls of the data model |
| `serde_json` | `JsonAction` and `YamlAction` carry a `serde_json::Value`, because a fix is data a host may serialize; the manifest values of a `Package` |
| `toml` | a rule's options: `RuleProvider::validate_options` receives the `toml::Table` |
| `indexmap` | the ordered maps of `Config` |
| `nodejs_semver` | the parsed ranges of a dependency specifier |

`oxc_*` and `rayon` types stay out of the public API, as ADR 0003 and [ADR 0010](0010-js-analysis-on-oxc.md) say; a thread pool is an `Executor`. A major release of a listed library is a breaking release of `skif`.

The list is written once, in the crate documentation of `skif` as `<!-- allow-list: ... -->` next to the table, and this record repeats it for the record. ADR 0003 stays as written; this record replaces its sentence and its rustdoc-JSON confirmation.

**The check.** `crates/skif/tests/public_api.rs` reads the sources of `skif` and of every `skif_*` crate it re-exports from. For each public item header (function and type signatures, fields, enum variants) it fails on a path that starts in a crate outside the allow-list, the standard library and skif, and on a name imported from such a crate. The scanner is conservative, since it also reads public items the facade does not re-export, and it has tests that prove it flags a leaking field and a leaking return type. It does not follow type aliases or macro output; the review of a re-export stays the backstop for those.

**The `semver` job stays advisory.** `cargo-semver-checks` compares against the latest release on crates.io, and nothing is published until 0.1.0. The job is skipped until then, so a blocking job would block on nothing; it becomes blocking when the first release is the baseline.

### Consequences

* Good, because the allow-list exists in one place and a test fails when a type outside it enters the API.
* Good, because the check runs on stable and needs no extra tool.
* Bad, because a source scan misses a leak behind a type alias or a macro.
* Bad, because the scanner reads the sources of the re-exported crates, so the test only runs in a checkout of the repository.

### Confirmation

* `crates/skif/tests/public_api.rs` passes and its negative tests flag a leaking field and a leaking return type.
* The allow-list in the `skif` crate documentation names the same libraries as the table above; the test compares them.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### State the allow-list once in the `skif` crate documentation, record it here, and audit the public sources in a stable test

* Good, because it is true now and enforced on the toolchain in use.
* Bad, because the audit is a heuristic over source text.

### Wait for a rustdoc-JSON test on a nightly toolchain

* Good, because rustdoc JSON sees every resolved type.
* Bad, because it needs a nightly toolchain and its format changes between releases, which is why ADR 0019 withdrew it.

### Keep ADR 0003 as written and rely on review

* Good, because it needs no work.
* Bad, because the record is already wrong and review has not caught it.

## More Information

* Amends: [ADR 0003](0003-library-first-embedding.md)
* Related decisions: [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md)
