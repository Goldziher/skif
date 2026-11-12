---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Specifier and Semver Model

## Context and Problem Statement

Every policy and several manifest rules compare dependency specifiers: is `^18.2.0` the same range type as `~18.2.0`,
does `18.3.1` satisfy a peer's `^17 || ^18`, does `workspace:^` point at the local version, what does `catalog:react18`
resolve to. A `package.json` specifier is not just a semver range. It can be an exact version, a range, a complex
range (`||`, hyphen), an x-range, `latest` or `*`, a dist-tag, `workspace:`, `catalog:`, an `npm:` alias, `file:` or
`link:`, a git or tarball URL, or something unsupported.

The Rust `semver` crate implements Cargo's requirement grammar, not npm's: it has no `||` and no hyphen ranges, and a
bare `1.2.3` means `^1.2.3` to Cargo and `=1.2.3` to npm. syncpack contains an MIT-licensed specifier model,
`syncpack-specifier`, that classifies every shape exactly as syncpack's groups need, but it is written for a
single-threaded CLI: it shares values through `Rc`, caches in thread-locals and depends on `node-semver`. How does skif
parse and compare specifiers?

## Decision Drivers

* npm range semantics, including `||` peer ranges
* Classification identical to syncpack's, which policy parity depends on
* Shareable across worker threads, with no global or thread-local state
* No user input can panic the parser
* Upstream changes are pulled deliberately and stay attributed

## Considered Options

* Vendor `syncpack-specifier` at a pinned commit into `skif-specifier`, on `nodejs-semver`, with a fixed modification
  list
* Keep the `semver` crate and pre-process npm syntax
* Depend on `syncpack-specifier` from crates.io
* Write a new specifier parser

## Decision Outcome

Chosen option: "Vendor `syncpack-specifier` at a pinned commit into `skif-specifier`, on `nodejs-semver`, with a fixed
modification list", because it gives npm range semantics and syncpack's exact classification with the smallest fork.

* `syncpack-specifier` is vendored at upstream commit `958d306` into the `skif-specifier` crate. The changes are the
  ones the modification list in `ATTRIBUTIONS.md` records, and no others:
  * `Rc` becomes `Arc`, and the thread-local caches are removed. Parsing does not cache; a caller-owned `SpecifierCache`
    interns parsed specifiers for whoever wants one. The crate keeps no global or thread-local state.
  * `node-semver` is replaced by `nodejs-semver`, which has the same npm range semantics.
  * Validation is eager: a number that does not fit (`99999999999999999999`) yields `Unsupported` instead of panicking
    later.
  * `Ord` breaks semantic ties on the raw string, so it agrees with `Eq` (`1.2.3` and `=1.2.3` do not compare equal),
    which makes sorting and ordered sets deterministic.
  * `Send + Sync` is asserted at compile time for the public types.
* `skif-specifier` is the only place where specifiers are parsed. Discovery parses each manifest entry once and shares
  the result with every rule ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)). The `semver` crate is
  never used for npm specifiers.
* The version-group state machine in `skif-rules` re-implements syncpack's visitors at the same commit on skif's own
  instance model; it copies no code ([ADR 0010](0010-version-semver-range-and-catalog-policies.md)).
* Every vendored file carries an attribution header naming its upstream path and commit, and `ATTRIBUTIONS.md` holds the
  syncpack section with the licence and the modification list ([ADR 0033](0033-attribution-and-vendoring.md)).
* A re-vendor is a single commit that moves the recorded commit, re-applies the modification list and updates the
  attribution.
* `mise run diff:syncpack` compares skif's classification and policy findings with the pinned syncpack
  ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)).

### Consequences

* Good, because `^17 || ^18` and hyphen ranges evaluate, so `incompatible-peer-dependencies` checks every peer range.
* Good, because policy findings classify specifiers exactly as syncpack does.
* Good, because specifiers are shared across workers without locks on the hot path.
* Good, because no specifier in a manifest can panic skif.
* Bad, because a fork must be re-vendored by hand to follow upstream fixes.
* Bad, because the vendored code follows syncpack's structure rather than skif's conventions, which the lint allowances
  in the crate document.

### Confirmation

* `scripts/check-headers.sh` checks every file under `crates/skif-specifier/src/` for its header and the commit recorded
  in `ATTRIBUTIONS.md`.
* `crates/skif-specifier/src/hardening_test.rs`: `out_of_range_numbers_nested_in_other_variants_do_not_panic` and
  `largest_safe_numbers_still_parse_and_do_not_panic`.
* `crates/skif-specifier/src/ordering_test.rs` and `crates/skif-specifier/src/eq_hash_test.rs`
  (`specifier_eq_by_raw_string`, `specifier_hash_consistent_with_eq`) cover the `Ord` and `Eq` laws.
* The policy conformance fixtures under `fixtures/policy`, run by `crates/skif/tests/policy_conformance.rs`, and
  `mise run diff:syncpack` hold the classification against syncpack.

## Pros and Cons of the Options

### Vendor `syncpack-specifier` at a pinned commit into `skif-specifier`, on `nodejs-semver`, with a fixed modification list

* Good, because it is the smallest change that makes the upstream model thread-safe and panic-free.
* Bad, because the vendored copy must be kept in step deliberately.

### Keep the `semver` crate and pre-process npm syntax

* Good, because it needs no new dependency.
* Bad, because rewriting npm ranges into Cargo's grammar is lossy, and the bare-version meaning differs.

### Depend on `syncpack-specifier` from crates.io

* Good, because upstream fixes arrive with a version bump.
* Bad, because the published crate keeps `Rc` and thread-local caches, so it cannot be shared across workers.

### Write a new specifier parser

* Good, because it would follow skif's conventions from the start.
* Bad, because it would diverge from syncpack's classification, which is the specification the policies follow.
