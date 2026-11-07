---
status: accepted
date: 2026-11-03
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0010"]
---

# oxc Lockstep With poly: One Release, Coordinated Bumps, a CI Build Against the Git Copy

## Context and Problem Statement

[ADR 0010](0010-js-analysis-on-oxc.md) pins every oxc crate exactly and notes that the pins conflict with a host on a different oxc, naming poly as the
example. That conflict is now a requirement: poly embeds skif's `js-analysis` in its web component, next to `oxc_linter` and the oxc formatters.
`oxc_linter` is unpublished and `oxc_formatter` is a `0.0.0` placeholder, so poly depends on oxc through a git `rev` at a release tag's commit.
A crates.io copy and a git copy of the same crate are distinct to cargo, so poly redirects skif's registry crates (`oxc_allocator`, `oxc_ast`,
`oxc_ast_visit`, `oxc_parser`, `oxc_semantic`, `oxc_span`, `oxc_str`) to its git copy with `[patch.crates-io]`. A patch applies only when the
patched version satisfies the requirement, and skif's caret requirements accept one oxc minor. Any skew between the two repositories therefore
turns into a resolution failure or a second copy of `Allocator` in poly's build.

## Decision Drivers

* poly must resolve skif's oxc crates to its own git copy, with one `Allocator` and one AST in the graph
* oxc breaks its API weekly, so drift should surface in skif's CI rather than in poly's
* [ADR 0010](0010-js-analysis-on-oxc.md) and [ADR 0003](0003-library-first-embedding.md) already keep oxc types out of the public API

## Considered Options

* Track the oxc release poly pins, bump in coordinated changes, and build skif against the git copy in CI
* Keep independent pins and leave poly to fix the mismatch
* Loosen skif's pins to caret ranges
* Depend on oxc by git `rev` in skif

## Decision Outcome

Chosen option: "Track the oxc release poly pins, bump in coordinated changes, and build skif against the git copy in CI", because it is the only option under which poly's `[patch.crates-io]` resolves and a drift is caught by skif's own CI.

* **One release.** skif's `oxc_allocator`, `oxc_ast`, `oxc_ast_visit`, `oxc_parser`, `oxc_semantic`, `oxc_span` and `oxc_str` pins equal the version of the oxc
  release (`crates_v<version>` in `oxc-project/oxc`) that poly pins by `rev`. The requirements are caret ranges as in ADR 0010.
* **Coordinated bumps.** An oxc bump is a pair of pull requests, one in skif and one in poly, opened together and merged skif first. Neither
  repository moves to a new oxc release alone. `oxc_resolver` is a separate project, is not part of the patch and keeps its own caret range.
* **CI.** The `oxc lockstep` workflow runs `scripts/check-oxc-git-patch.sh` weekly and on changes to `Cargo.toml` or `Cargo.lock`. The script resolves the
  tag's commit with `git ls-remote`, patches the seven crates to it with `--config 'patch.crates-io.<crate>.git=…'` flags (no file is edited), builds
  `skif` with `js-analysis`, and fails when cargo reports an unused patch, when two copies of an oxc crate remain, or when a crate does not resolve
  to the git copy. `mise run oxc:lockstep` runs it locally, and an optional tag argument tries the next release before bumping.
* **Public API.** No oxc type appears in a public item of any skif crate, as ADR 0010 and ADR 0003 require. A host that patches oxc therefore sees no
  oxc type from skif and shares only the crate graph.

### Consequences

* Good, because poly's patch resolves, and drift between the two repositories fails skif's scheduled CI first.
* Good, because the weekly run against the next tag shows upgrade breakage before the bump pull request.
* Bad, because skif's oxc upgrades are gated on poly's release cadence, and the weekly oxc breakage becomes a cross-repository change.
* Bad, because the check needs network access to github.com and builds oxc from source, so it is slower than a registry build.

### Confirmation

* `.github/workflows/oxc-lockstep.yml` passes on `main` and fails on a deliberately skewed pin (for example, `oxc_span` one version behind).
* `scripts/check-pins.sh` requires a caret range for every `oxc*` workspace dependency and one release across the lockstep crates.
* The `skif-js-syntax` rustdoc-JSON check from ADR 0010 continues to assert that no public item names an `oxc_*` type.

## Pros and Cons of the Options

### Track the oxc release poly pins, bump in coordinated changes, and build skif against the git copy in CI

* Good, because it matches what poly's patch needs and detects drift mechanically.
* Bad, because every oxc bump needs two pull requests.

### Keep independent pins and leave poly to fix the mismatch

* Good, because skif upgrades on its own schedule.
* Bad, because the patch does not apply when versions differ, so poly links two incompatible copies of oxc.

### Loosen skif's pins to caret ranges

* Good, because a range admits poly's version within one oxc minor, and `cargo upgrade` handles it without `--pinned`.
* Neutral, because below 1.0 a caret stops at the next minor, so it never admits oxc's breaking releases. This option is taken alongside the chosen one.

### Depend on oxc by git `rev` in skif

* Good, because it names the same source as poly.
* Bad, because skif's crates are published to crates.io, which forbids git dependencies, and four oxc crates are not published at all.

## More Information

* Amends: [ADR 0010](0010-js-analysis-on-oxc.md), the consequence that exact oxc pins conflict with a host on a different oxc
* Related decisions: [ADR 0003](0003-library-first-embedding.md)
* Upstream: [oxc](https://github.com/oxc-project/oxc), tags `crates_v<version>`
* See the [ADR index](index.md).
