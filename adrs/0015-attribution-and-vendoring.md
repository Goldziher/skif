---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Attribution and Vendoring: Vendor, Re-Implement or Depend, and Credit Every Source

## Context and Problem Statement

skif draws on four upstream projects in four different ways:

| Project | License | Relationship |
| --- | --- | --- |
| sherif (QuiiBz/sherif) | MIT, © 2023 Tom Lienard | this repository is a fork; some rule logic and fixtures are ported |
| syncpack (JamieMason/syncpack) | MIT, © 2017-2023 Jamie Mason | `syncpack-specifier` and `pattern_matcher` vendored ([ADR 0006](0006-specifier-and-semver-model.md), [ADR 0009](0009-version-and-catalog-policies.md)); group semantics re-implemented |
| knip (webpro-nl/knip) | ISC, © 2022-2026 Lars Kappert | semantics re-implemented as a specification ([ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md)); fixtures may be adapted |
| fallow (fallow-rs/fallow) | MIT | design reference only |

MIT and ISC both require the copyright notice and permission text to accompany copies or
substantial portions of the software. Without a rule for when code is copied and how it is
credited, a fork that is being rebuilt drifts into an unclear state: ported functions without
headers, fixtures whose origin nobody remembers, and a `LICENSE` that still names the upstream
author for code that is no longer theirs.

## Decision Drivers

* MIT and ISC require the copyright notice and permission text to accompany copied code
* A fork being rebuilt drifts into ported functions without headers and fixtures of unknown origin
* skif's own `LICENSE` must not name upstream authors for code that is no longer theirs

## Considered Options

* Depend, vendor, re-implement or reference only, with per-file headers and ATTRIBUTIONS.md
* Keep upstream notices in `LICENSE`
* A `third_party/` directory for all vendored code
* Depend on syncpack's crates and patch them with `[patch]`

## Decision Outcome

Chosen option: "Depend, vendor, re-implement or reference only, with per-file headers and ATTRIBUTIONS.md", because a concrete question per upstream, headers in every file with copied code and one auditable attribution file keep every copied line traceable and meet the MIT and ISC obligations.

### Vendor, re-implement or depend

* **Depend** on a published crate when its API fits skif's constraints as is ([ADR 0003](0003-library-first-embedding.md)): thread
  safety, no global state, no IO in library code. `oxc_*`, `oxc_resolver`, `jsonc-parser`,
  `yamlpatch` and `nodejs-semver` are dependencies.
* **Vendor** when the code is right but its shape is not, and the needed changes are not upstream
  concerns — `syncpack-specifier` needs `Arc` and a caller-owned cache that a single-threaded CLI
  does not ([ADR 0006](0006-specifier-and-semver-model.md)). Vendored code lives in its own crate or module, unchanged except for a
  recorded list of modifications, so a re-vendor is mechanical.
* **Re-implement** when what skif needs is the behaviour, not the code, or the code is in another
  language: syncpack's group state machine and all of knip. The upstream is then the
  **specification**, cited by commit, and its observed output on the corpus is the oracle.
* **Reference only** means reading for ideas with no code, tests or data copied: fallow.

### Per-file headers

Every file containing vendored or ported code starts with a header naming the upstream, the file it
came from, the commit and the license:

```rust
// Vendored from syncpack (https://github.com/JamieMason/syncpack), crates/syncpack-specifier/src/lib.rs
// at 958d306. MIT License, Copyright (c) 2017-2023 Jamie Mason. Modified; see ATTRIBUTIONS.md.
```

A test or fixture adapted from knip carries the same header (or a sibling `ATTRIBUTION` note for
data files that cannot hold comments), naming the knip fixture and commit
`da07b6e9887f9a1eb08545da29755b7f9d309bca`.

### ATTRIBUTIONS.md

`ATTRIBUTIONS.md` at the repository root has one section per upstream, in the format basemind and
poly use: source URL, upstream commit, license, authors, location in this repository, the **full
upstream license text**, and a `### Modifications` list. A section is added in the same commit that
first copies code or data from that upstream, and removed only when nothing from it remains.
Re-implemented specifications (syncpack's semantics, knip's semantics) are credited in
`ATTRIBUTIONS.md` under "Specifications" even though no code is copied, because the tests encode
their behaviour.

### LICENSE

skif's own `LICENSE` is MIT, `Copyright (c) 2026 Na'aman Hirschfeld`. Upstream copyright notices are
not merged into it; they live in `ATTRIBUTIONS.md` with their full license text, alongside the code
they cover.

### Consequences

* Good, because every copied line can be traced to a source, commit and license from the file itself.
* Good, because the license obligations of MIT and ISC are met in one auditable file, and a release ships it.
* Good, because the vendor / re-implement / depend rule gives a concrete question for each new upstream instead of
  a case-by-case argument.
* Bad, because headers and modification lists must be maintained on every re-vendor; a reviewer checks them.
* Bad, because porting sherif logic piecemeal means each ported file must be identified at port time; anything
  rewritten from scratch carries no header, and the boundary is a judgement made in review.
* Bad, because "Reference only" for fallow relies on discipline, not tooling. Contributors who have read fallow's
  code write skif's from the specification and ADRs, not from memory of fallow's implementation.

### Confirmation

* Reviewers check headers and modification lists on every vendor or re-vendor.
* `ATTRIBUTIONS.md` gets a section in the same commit that first copies code or data from an upstream, and a release ships it.
* Boundaries between ported and rewritten code are judged in review.

## Pros and Cons of the Options

### Depend, vendor, re-implement or reference only, with per-file headers and ATTRIBUTIONS.md

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Keep upstream notices in `LICENSE`

* Bad, because it conflates skif's license with the notices of code it contains, and grows unreadable as upstreams are added.

### A `third_party/` directory for all vendored code

* Bad, because vendored code must sit in the crate that compiles it ([ADR 0002](0002-crate-layout-and-publishing.md)); headers plus `ATTRIBUTIONS.md` give the same traceability.

### Depend on syncpack's crates and patch them with `[patch]`

* Bad, because a patched dependency cannot be published to crates.io, which every skif crate is ([ADR 0002](0002-crate-layout-and-publishing.md)).

## More Information

* Related decisions: [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md)
* Upstream: [sherif](https://github.com/QuiiBz/sherif)
* Upstream: [syncpack](https://github.com/JamieMason/syncpack)
* Upstream: [knip](https://github.com/webpro-nl/knip)
* Upstream: [fallow](https://github.com/fallow-rs/fallow)
* See the [ADR index](index.md).
