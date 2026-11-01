---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Specifier and Semver Model: Vendored syncpack-specifier on nodejs-semver

## Context and Problem Statement

Every policy ([ADR 0009](0009-version-and-catalog-policies.md)) and several manifest rules compare dependency specifiers: is `^18.2.0`
the same range type as `~18.2.0`, does `18.3.1` satisfy a peer's `^17 || ^18`, is
`workspace:^` pointing at the local version, what does `catalog:react18` resolve to. A
`package.json` specifier is not just a semver range. It can be an exact version, a range, a complex
range (`||`, hyphen), an x-range or bare major/minor, `latest` or `*`, a dist-tag, `workspace:`,
`catalog:`, an `npm:` alias, `file:` or `link:`, a git URL, a tarball URL, or something
unsupported.

The inherited code parses specifiers with the `semver` crate (`packages/semversion.rs`), which
implements **Cargo's** requirement grammar, not npm's:

* no `||` — `^17 || ^18`, the most common shape of a `peerDependencies` range, does not parse;
* no hyphen ranges (`1.2.3 - 2.3.4`);
* a bare `1.2.3` means `^1.2.3` to Cargo and `=1.2.3` to npm;
* comparators are comma-separated, where npm separates them with spaces.

The visible symptom: `incompatible-peer-dependencies` calls `SemVersion::parse(...).ok()?` on the
peer range (`packages/mod.rs`), so a peer range with `||` silently disables the check for that
dependency instead of evaluating it.

syncpack (v13+, Rust) already contains a specifier model that classifies every specifier shape
and answers the questions its version and semver groups need: `syncpack-specifier`, MIT licensed.
It is written for a single-threaded CLI: it shares values through `Rc` and caches parsed
specifiers in thread-locals, and it depends on the `node-semver` crate.

## Decision Drivers

* Policies and manifest rules must compare npm specifiers correctly
* The `semver` crate implements Cargo's grammar, so `^17 || ^18` peer ranges silently skip `incompatible-peer-dependencies`
* skif must share specifiers across rayon workers with no global or thread-local state
* Policy parity with syncpack depends on classifying specifiers exactly as it does

## Considered Options

* Vendor syncpack-specifier at commit 958d306 into skif-specifier, on nodejs-semver
* Keep the `semver` crate and pre-process npm syntax
* Depend on `syncpack-specifier` from crates.io
* Write a new specifier parser

## Decision Outcome

Chosen option: "Vendor syncpack-specifier at commit 958d306 into skif-specifier, on nodejs-semver", because it gives npm range semantics (including `||` peer ranges) and syncpack's exact specifier classification with the smallest fork, once `Rc`, thread-local caches and `node-semver` are replaced.

skif **vendors `syncpack-specifier` at upstream commit `958d306`** into the `skif-specifier`
crate, with these modifications and no others:

* `Rc` → `Arc`, so specifiers can be shared across rayon workers.
* Thread-local caches → a caller-owned `SpecifierCache` passed explicitly (one per lint run, held
  by the `LintContext`, [ADR 0007](0007-rule-and-diagnostic-model.md)). No global or thread-local state remains ([ADR 0003](0003-library-first-embedding.md)).
* `node-semver` → `nodejs-semver`, a fork of `node-semver` with the same npm range semantics
  (`||`, hyphen ranges, x-ranges, npm prerelease matching).
* `Send + Sync` asserted at compile time for the public types.
* An attribution header on every vendored file and a syncpack section in `ATTRIBUTIONS.md` with
  the upstream URL, commit, license text and a `### Modifications` list ([ADR 0015](0015-attribution-and-vendoring.md)).

`skif-specifier` is the only place specifiers are parsed. `skif-workspace` attaches a parsed
`Specifier` to every `DependencyInstance` ([ADR 0005](0005-workspace-discovery.md)), and every rule that compares versions goes
through it. No crate depends on the `semver` crate for npm specifiers.

Upstream changes are pulled deliberately: a re-vendor is a single commit that moves the recorded
commit, re-applies the modification list, and updates the attribution.

### Consequences

* Good, because skif classifies specifiers exactly as syncpack does, which is what makes syncpack's group
  semantics ([ADR 0009](0009-version-and-catalog-policies.md)) reproducible and lets `mise run diff:syncpack` compare results directly.
* Good, because npm range semantics are correct, including `||` peer ranges; the silent-skip bug class goes away.
* Good, because the cache is explicit, so its lifetime and memory are visible to the host, and parallel lint
  runs cannot share or poison each other's state.
* Bad, because vendored code is code skif maintains. Upstream fixes do not arrive automatically, and a re-vendor
  has to re-apply the `Arc` and cache changes.
* Bad, because `nodejs-semver` and `node-semver` may diverge in edge cases; the vendored tests run against
  `nodejs-semver` and any behavioural difference is recorded in the modification list.
* Bad, because `Arc` costs an atomic increment where `Rc` did not. Specifiers are created once per dependency
  instance and cached, so the cost is per distinct specifier, not per comparison.

### Confirmation

* The vendored tests run against `nodejs-semver`, and any behavioural difference is recorded in the modification list.
* `Send + Sync` is asserted at compile time for the public types.
* `mise run diff:syncpack` compares skif's results with syncpack's directly.
* Every vendored file carries an attribution header and `ATTRIBUTIONS.md` lists the modifications ([ADR 0015](0015-attribution-and-vendoring.md)).

## Pros and Cons of the Options

### Vendor syncpack-specifier at commit 958d306 into skif-specifier, on nodejs-semver

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Keep the `semver` crate and pre-process npm syntax

* Bad, because translating `||`, hyphen ranges and bare-version semantics into Cargo's grammar is a parser in itself, and gets prerelease matching wrong anyway.

### Depend on `syncpack-specifier` from crates.io

* Bad, because the `Rc` and thread-local design is incompatible with [ADR 0003](0003-library-first-embedding.md), and the changes are not upstream concerns for a single-threaded CLI. Vendoring is the smaller fork.

### Write a new specifier parser

* Bad, because syncpack's classification is the specification the policies follow; re-deriving it would introduce differences for no gain.

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0015](0015-attribution-and-vendoring.md)
* Upstream: [syncpack](https://github.com/JamieMason/syncpack)
* See the [ADR index](index.md).
