---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Corrections to ADR 0044 and ADR 0042: Config Helpers Are Not Built, Referenced Paths Are Scoped to Listed Packages, a Zero Trailer Does Not Detect a Torn Read

## Context and Problem Statement

[ADR 0044](0044-monorepo-js-corrections-builtins-html-entries-referenced-paths-extglob-helpers-and-stylesheets.md) records the crediting of a shared configuration helper as a decision, and the release notes listed it as fixed. Commit `6474953` reverted that behaviour, because the crediting could not be limited to the files a helper names, and left the record, the confirmation and one changelog line describing it. Its regression test is `#[ignore]`d. A second regression test of the same record, for the `paths` of a referenced `tsconfig`, is also `#[ignore]`d: it holds for a package that lists the referenced package as a dependency and fails for one that does not.

[ADR 0042](0042-git-index-discovery-corrections-directory-times-lazy-existence-and-checksum.md) states that the reader never trusts a torn or concurrently rewritten index, while accepting an all-zero trailer, which git writes under `index.skipHash` (set by `feature.manyFiles`). For the repositories that setting is meant for, the trailer checks nothing.

Smaller drifts found in the same review: [ADR 0017](0017-release-policy-a-single-complete-0-1-0.md) names the floating tag `v0`, which [ADR 0043](0043-floating-tags-for-0x-releases-and-musl-binstall.md) stopped moving; [ADR 0002](0002-crate-layout-and-publishing.md) counts fourteen crates while the repository holds fourteen published crates and the unpublished `skif-node`; the documentation claimed a speed-up that no published measurement supports; the resolvers of a run each kept their own file-system cache.

## Considered Options

* Leave the records as written and rely on the changelog
* Supersede the statements that do not hold, in one new record and in the index, and correct the documentation
* Rebuild the config-helper crediting before the first release

## Decision Outcome

Chosen option: "Supersede the statements that do not hold, in one new record and in the index, and correct the documentation", because the records are locked and the helper crediting needs its own design: crediting a package wholesale hides real findings, and crediting only the files a helper names requires evaluating the helper.

* **Config helpers are not built.** The "Config helpers" paragraph of ADR 0044 and its confirmation do not describe skif. A configuration that evaluates to something unknown marks only its own package's entries incomplete. A helper imported from a sibling package credits nothing in that package. The ignored regression test stays as the specification for a later design.
* **Referenced `tsconfig` paths.** The statement of ADR 0044 holds for a package that lists the package owning the referenced config as a dependency. A package that does not list it still resolves its imports through the referenced paths. The ignored regression test marks the gap.
* **Floating tag in the documentation.** The Action is named `Goldziher/skif@v0.1` everywhere ([ADR 0043](0043-floating-tags-for-0x-releases-and-musl-binstall.md)); ADR 0017 and ADR 0013 keep their wording and the index says which statement governs.
* **Torn reads.** The SHA-1 trailer detects a torn or concurrently rewritten index only when git wrote one. With an all-zero trailer the reader relies on its bounds checks alone, and the configuration reference says so.
* **Shared resolver cache.** Every set of resolvers of a run (scripts, per-package bundler extensions, styles) is built from one `oxc_resolver` cache, so a path is looked up on disk once however many sets ask for it.
* **Crate count.** The workspace holds fourteen published crates ([ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md) added `skif-migrate`) and the unpublished `skif-node`. `scripts/test-release-scripts.sh` checks that `scripts/publish-crates.sh` publishes every crate after the skif crates it depends on.
* **Speed claims.** The documentation states no speed-up until a measurement is published with its date and tree.
* **Public API.** The result types of `explain` and of `skif-migrate`, and `PathKind`, are `#[non_exhaustive]`, so adding a field or a variant is not a breaking change; `skif-migrate` offers constructors for its input types. The facade re-exports `PathKind` and the types of `UsageReport`. `skif-js`, `skif-js-plugins`, `skif-js-syntax` and `skif-edit` warn on missing documentation like the other crates. The third-party audit of `crates/skif/tests/public_api.rs` reads trait members and `pub use` re-exports and covers every crate the facade builds on.

### Consequences

* Good, because the records, the changelog and the documentation say what the code does.
* Good, because the reasons for the two ignored tests are recorded where a later change will find them.
* Good, because a host can name every type the facade returns and can rely on later minor versions adding to them.
* Bad, because a package that wraps its configuration in a shared helper can still be reported for files and dependencies only the helper names, until the helper design is done.
* Bad, because an index written with `index.skipHash` has no defence against a torn read other than the reader's bounds checks.

### Confirmation

* `scripts/test-release-scripts.sh` runs `scripts/release-floating-tag.sh` against a bare remote for `0.1.0`, `0.10.3`, `1.2.3`, a pre-release and a dry run, and checks the publish order against the dependency graph.
* `crates/skif-js/tests/monorepo_regressions.rs` holds the two ignored cases; they run when the behaviour is built.
* `crates/skif-js/src/resolve.rs` has a test with a counting file system that no path is asked for its kind twice across the resolver sets of a run.
* `crates/skif/tests/public_api.rs` has negative cases for a foreign type in a trait method, an associated type and a re-export.
* `adrlint lint adrs` and `adrlint lock --check` pass.
