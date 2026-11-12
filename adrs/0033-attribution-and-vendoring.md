---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Attribution and Vendoring: Depend, Vendor, Re-Implement or Reference, and One Attributions File

## Context and Problem Statement

skif draws on upstream projects in different ways. It began as a fork of sherif (MIT) and ports its workspace
and manifest rules. It vendors syncpack's specifier crate (MIT) and re-implements syncpack's version-group semantics.
It re-implements knip's analysis (ISC) in Rust and adapts a little of its data. It reads fallow for ideas. MIT and
ISC both require the copyright notice and permission text to accompany copies or substantial portions. Without a rule
for when code is copied and how it is credited, a rebuilt fork drifts into ported code without notices, data of
unknown origin, and a `LICENSE` that names upstream authors for code that is no longer theirs.

How does skif decide how to use upstream code, and how is it credited?

## Decision Drivers

* MIT and ISC notices accompany everything copied or derived, in every published artefact
* Every copied line can be traced to its upstream, commit and licence
* skif's own `LICENSE` names skif's author only
* A re-vendor is mechanical
* What can be checked mechanically is checked in CI

## Considered Options

* Depend, vendor, re-implement or reference only, with headers on vendored files and one `ATTRIBUTIONS.md`
* Keep upstream notices in `LICENSE`
* A `third_party/` directory for all upstream code
* Depend on syncpack's crates and patch them with `[patch]`

## Decision Outcome

Chosen option: "Depend, vendor, re-implement or reference only, with headers on vendored files and one
`ATTRIBUTIONS.md`", because it gives a concrete question for each upstream and keeps every licence obligation in one
auditable file that every release ships.

### Four relationships

* Depend on a published crate when its API fits skif's constraints as it is
  ([ADR 0003](0003-library-api-and-embedding.md)): thread safety, no global state, no I/O in library code. oxc and
  `oxc_resolver`, `jsonc-parser`, `yamlpatch` and `nodejs-semver` are dependencies, governed by `deny.toml` like any
  other.
* Vendor when the code is right but its shape is not, and the changes are not upstream concerns: `syncpack-specifier`
  lives in `skif-specifier`, because skif needs `Arc` and a caller-owned cache
  ([ADR 0007](0007-specifier-and-semver-model.md)). Vendored code is unchanged except for a recorded list of
  modifications.
* Re-implement when skif needs the behaviour, not the code, or the code is in another language: syncpack's
  version-group state machine and all of knip. The upstream at a recorded commit is the specification, and its
  observed output on the corpus is the oracle ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)).
* Reference only means reading for ideas with no code, tests or data copied: fallow.

### Headers and `ATTRIBUTIONS.md`

* Every vendored file starts with a header naming the upstream, the file, the commit and the licence:
  `// Vendored from syncpack ...`, or `// Added for skif (not in syncpack) ...` for a file of the vendored crate that
  has no upstream counterpart.
* Code derived from sherif and knip is ported or re-implemented, not vendored file by file, and carries no header;
  its `ATTRIBUTIONS.md` section covers it. A test or fixture adapted from knip names the knip fixture and commit in a
  header, or in a sibling `ATTRIBUTION` note for data that cannot hold comments.
* `ATTRIBUTIONS.md` has one section per upstream that code or data came from: source URL, upstream commit, licence,
  authors and copyright, location in this repository, purpose, a `### Modifications` list and the full licence text.
  Re-implemented semantics are listed in the section of their upstream, and the modification lists say that no code
  was copied. A section is added in the change that first copies from an upstream and removed only when nothing from
  it remains.
* skif's `LICENSE` is MIT, `Copyright (c) 2026 Na'aman Hirschfeld`. Upstream notices are never merged into it.
* Every published crate includes `LICENSE` and `ATTRIBUTIONS.md`
  ([ADR 0002](0002-crates-publishing-and-repository-consistency-checks.md)), and every release archive ships them
  ([ADR 0027](0027-distribution-and-release.md)).
* A dependency whose licence is not on the `deny.toml` allow-list is avoided rather than excepted, as `notify` is for
  watch mode ([ADR 0028](0028-editors-agents-and-git-hooks.md)).

### Consequences

* Good, because every vendored line can be traced to a source, commit and licence from the file itself.
* Good, because the MIT and ISC obligations are met in one file that every crate and archive ships.
* Good, because each new upstream gets a concrete question instead of a case-by-case argument.
* Bad, because headers and modification lists must be maintained on every re-vendor.
* Bad, because the boundary between ported and rewritten sherif code, and "reference only" for fallow, rest on review
  and discipline, not tooling.

### Confirmation

* `scripts/check-headers.sh` (the `consistency` job and `mise run consistency`) fails when a file of
  `crates/skif-specifier/src/` lacks a header, when a header names a commit other than the one `ATTRIBUTIONS.md`
  records for syncpack, when a section or a `### Modifications` list is missing, or when the knip section lacks its
  commit, copyright or licence; `scripts/test-check-scripts.sh` tests it.
* `scripts/check-archive.sh` requires the binary, `LICENSE` and the attributions in every release archive, and each
  crate's `include` names `LICENSE` and `ATTRIBUTIONS.md`.

## Pros and Cons of the Options

### Depend, vendor, re-implement or reference only, with headers on vendored files and one `ATTRIBUTIONS.md`

* Good, because the relationship to each upstream is explicit and its obligations are in one place.
* Bad, because two of the four relationships are judged in review rather than checked.

### Keep upstream notices in `LICENSE`

* Good, because there is one licence file.
* Bad, because it names upstream authors for skif's own code, and the notices are separated from the code they
  cover.

### A `third_party/` directory for all upstream code

* Good, because copied code is easy to find.
* Bad, because the vendored specifier belongs in its own crate, and ported or re-implemented code has no single file
  to move.

### Depend on syncpack's crates and patch them with `[patch]`

* Good, because there is no copy to maintain.
* Bad, because a `[patch]` does not apply to published crates, and the changes skif needs are not upstream concerns.
