---
status: accepted
date: 2026-11-02
decision-makers: ["Na'aman Hirschfeld"]
---

# Reality Reconciliation: Crate List, Schema File, Test Tooling and Consistency Checks

## Context and Problem Statement

Accepted records are locked, so statements in them that the repository did not turn out to follow can only be
corrected by a later record. Several have drifted: [ADR 0002](0002-crate-layout-and-publishing.md) lists a `skif-testing` crate that does not exist and a
publish order that includes a `skif-js` crate that has not landed; [ADR 0004](0004-configuration-model.md) promises a Rust test that
compares `schema/skif.schema.json` with the generated schema; [ADR 0012](0012-reporting.md) and [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) name `insta-cmd`
for CLI tests; [ADR 0001](0001-mission-and-scope.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md) and [ADR 0011](0011-plugin-model.md) name `mise run parity` and
`mise run diff:syncpack`; and [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0010](0010-js-analysis-on-oxc.md) and [ADR 0015](0015-attribution-and-vendoring.md) describe `consistency`
checks, clippy bans and compile-time assertions of which only some exist. Each statement has to be either made
true or dropped explicitly, so that a reader of the records knows what is enforced.

## Decision Drivers

* A confirmation line in a record must describe something that runs
* Cheap checks should exist rather than be promised
* A claim that is not worth a check should be withdrawn rather than left standing
* Locked records stay unedited; corrections live in a new record

## Considered Options

* Reconcile in one record: implement the cheap checks, state the real tooling and withdraw the rest
* Leave the records as written and treat the code as the authority
* Implement every promised check before the first release

## Decision Outcome

Chosen option: "Reconcile in one record: implement the cheap checks, state the real tooling and withdraw the rest", because it makes the confirmation lines of the earlier records true or void without waiting for expensive tooling.

**Crates.** The workspace contains `skif-core`, `skif-specifier`, `skif-config`, `skif-edit`, `skif-workspace`,
`skif-lint`, `skif-rules`, `skif-js-syntax`, `skif-js-plugins`, `skif-report`, `skif` and `skif-cli`. There is no
`skif-testing` crate and none is planned: fixtures and redaction helpers live next to the tests that use them, and
every crate under `crates/` is published. `skif-js` is added before 0.1.0 ([ADR 0017](0017-release-policy-a-single-complete-0-1-0.md)); `scripts/publish-crates.sh` already
lists it and skips a crate whose directory does not exist. `tools/bench` (`skif-bench`) is `publish = false`.

**Published crates carry their notices.** Every crate under `crates/` holds a copy of the root `LICENSE` and
`ATTRIBUTIONS.md` and lists them in its `include`, so the MIT notices for the vendored syncpack code and the
sherif-derived rules travel with each package ([ADR 0015](0015-attribution-and-vendoring.md)).

**Schema file.** `schema/skif.schema.json` is committed and is what the `#:schema` directive written by `skif init`
points to. Its freshness is checked by `scripts/check-schema.sh`, which diffs the file against the output of
`skif schema`; it runs as the mise task `schema:check` and as a step of the CI `build` job. This replaces the Rust
test promised in [ADR 0004](0004-configuration-model.md). The file is refreshed with `cargo run -p skif-cli -- schema > schema/skif.schema.json`.

**CLI test tooling.** CLI tests use `assert_cmd` to run the built binary and `insta` snapshots for its output
(`crates/skif-cli/tests`). The statements in [ADR 0012](0012-reporting.md) and [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) that name `insta-cmd` read as this combination.

**Corpus comparison.** There are no `mise run parity` or `mise run diff:syncpack` tasks. The pinned corpus is
`tools/bench/corpus.toml`, fetched by `mise run bench:corpus`, and `mise run bench` measures skif against the other
tools that are installed. Parity with knip and syncpack is a number reported by that harness once the analysis
phase lands, not a separate task and not a release gate; every sentence in earlier records that makes a release
depend on `parity` or `diff:syncpack` is withdrawn.

**Checks that exist.** These run in the `consistency` job of `.github/workflows/ci.yml` (and as `mise run consistency`):

| Script | Enforces |
| --- | --- |
| `scripts/check-versions.sh` | one version across crates, internal pins, npm packages, hook pins and the pre-commit launcher ([ADR 0013](0013-distribution-and-naming.md), [ADR 0014](0014-git-hook-integration.md)) |
| `scripts/check-pins.sh` | every `oxc_*` entry in `[workspace.dependencies]` is pinned with `=` ([ADR 0010](0010-js-analysis-on-oxc.md)); every dependency of a member crate is `dep.workspace = true` ([ADR 0002](0002-crate-layout-and-publishing.md)) |
| `scripts/check-headers.sh` | every file under `crates/skif-specifier/src/` starts with a `Vendored from syncpack` or `Added for skif` header, headers name the commit recorded in `ATTRIBUTIONS.md`, and `ATTRIBUTIONS.md` keeps its sections and `Modifications` lists ([ADR 0006](0006-specifier-and-semver-model.md), [ADR 0015](0015-attribution-and-vendoring.md)) |
| `scripts/check-package-contents.sh` | each crate's `LICENSE` and `ATTRIBUTIONS.md` equal the root copies and appear in `cargo package --list` |
| `scripts/check-rule-ids.sh` | every id in the committed `rule-ids.txt` is still documented in `docs/rules.md`, and every documented id is recorded ([ADR 0007](0007-rule-and-diagnostic-model.md)) |
| `scripts/check-dependency-tree.sh` | `skif` without default features has no `oxc_*` crate, `skif-core` depends on no workspace crate, and only `skif-js-syntax`, `skif-js-plugins` and `skif-js` depend on oxc ([ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0010](0010-js-analysis-on-oxc.md)) |

`rule-ids.txt` replaces the registry-enumerating test of [ADR 0007](0007-rule-and-diagnostic-model.md): `docs/rules.md` is generated from the registry
and its freshness is already tested, so a rule that disappears from the registry disappears from the document and
fails the script. The `deny` job additionally bans duplicate versions (with documented skips), unknown registries
and unknown git sources, and also runs on a schedule.

**Checks that are withdrawn.** These confirmation lines do not apply:

* the `clippy.toml` `disallowed-methods` list and the `print_stdout` and `print_stderr` denials of [ADR 0003](0003-library-first-embedding.md): the I/O boundary is held by the `FileSystem` trait, by `OsFs` being the only library type that touches `std::fs`, and by review;
* the rustdoc-JSON test over public signatures ([ADR 0003](0003-library-first-embedding.md), [ADR 0011](0011-plugin-model.md)): the facade's re-exports are reviewed, and leaks of `toml` and `serde_json` types are tracked as defects;
* the `consistency` comparison of state names with a syncpack checkout ([ADR 0009](0009-version-and-catalog-policies.md)) and of plugin rows with a knip commit ([ADR 0011](0011-plugin-model.md)): the state table is held by the policy conformance fixtures under `fixtures/policy`, and plugin rows are checked when the plugin table exists;
* the check that adapted knip fixtures carry headers ([ADR 0015](0015-attribution-and-vendoring.md)) applies from the first such fixture on.

**`Send + Sync` assertions.** A compile-time assertion is required in each crate that defines a type shared between
rayon workers or exposed to hosts as a long-lived value (`skif-core`, `skif-specifier`, `skif-config`, `skif-edit`,
`skif-lint`, `skif-rules`, `skif-js-syntax`); it is not required in a crate that has no such type.

### Consequences

* Good, because every confirmation line in the earlier records is either backed by a script or explicitly withdrawn.
* Good, because the new checks are plain shell over `Cargo.toml`, `ATTRIBUTIONS.md` and `docs/rules.md`, so they run in seconds.
* Bad, because the records now have to be read together with this one to know the current rule.
* Bad, because shell checks over TOML are textual and need maintenance if the manifests change shape.

### Confirmation

* `mise run consistency` runs the six scripts above and passes on a clean checkout; the CI `consistency` job runs the same scripts.
* `mise run schema:check` passes, and the CI `build` job runs the same script against the freshly built binary.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.
* `cargo package --list -p <crate>` contains `LICENSE` and `ATTRIBUTIONS.md` for every crate under `crates/`.

## Pros and Cons of the Options

### Reconcile in one record: implement the cheap checks, state the real tooling and withdraw the rest

* Good, because it ends the gap between what the records say and what runs, at the cost of a handful of small scripts.
* Bad, because it withdraws some checks that would have been valuable.

### Leave the records as written and treat the code as the authority

* Good, because it needs no work.
* Bad, because the records stop being a reliable description of what is enforced.

### Implement every promised check before the first release

* Good, because every promise would be kept.
* Bad, because several checks (a rustdoc-JSON signature test, a syncpack-checkout comparison) are costly for little protection beyond what review and the conformance fixtures give.

## More Information

* Amends: [ADR 0001](0001-mission-and-scope.md), [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md), [ADR 0012](0012-reporting.md), [ADR 0015](0015-attribution-and-vendoring.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* See the [ADR index](index.md).
