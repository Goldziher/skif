# 0006 — Specifier and Semver Model: Vendored syncpack-specifier on nodejs-semver

* Status: Accepted
* Date: 2026-11-01

## Context

Every policy (ADR 0009) and several manifest rules compare dependency specifiers: is `^18.2.0`
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

## Decision

skif **vendors `syncpack-specifier` at upstream commit `958d306`** into the `skif-specifier`
crate, with these modifications and no others:

* `Rc` → `Arc`, so specifiers can be shared across rayon workers.
* Thread-local caches → a caller-owned `SpecifierCache` passed explicitly (one per lint run, held
  by the `LintContext`, ADR 0007). No global or thread-local state remains (ADR 0003).
* `node-semver` → `nodejs-semver`, a fork of `node-semver` with the same npm range semantics
  (`||`, hyphen ranges, x-ranges, npm prerelease matching).
* `Send + Sync` asserted at compile time for the public types.
* An attribution header on every vendored file and a syncpack section in `ATTRIBUTIONS.md` with
  the upstream URL, commit, license text and a `### Modifications` list (ADR 0015).

`skif-specifier` is the only place specifiers are parsed. `skif-workspace` attaches a parsed
`Specifier` to every `DependencyInstance` (ADR 0005), and every rule that compares versions goes
through it. No crate depends on the `semver` crate for npm specifiers.

Upstream changes are pulled deliberately: a re-vendor is a single commit that moves the recorded
commit, re-applies the modification list, and updates the attribution.

## Consequences

Positive:

* skif classifies specifiers exactly as syncpack does, which is what makes syncpack's group
  semantics (ADR 0009) reproducible and lets `mise run diff:syncpack` compare results directly.
* npm range semantics are correct, including `||` peer ranges; the silent-skip bug class goes away.
* The cache is explicit, so its lifetime and memory are visible to the host, and parallel lint
  runs cannot share or poison each other's state.

Negative / risks:

* Vendored code is code skif maintains. Upstream fixes do not arrive automatically, and a re-vendor
  has to re-apply the `Arc` and cache changes.
* `nodejs-semver` and `node-semver` may diverge in edge cases; the vendored tests run against
  `nodejs-semver` and any behavioural difference is recorded in the modification list.
* `Arc` costs an atomic increment where `Rc` did not. Specifiers are created once per dependency
  instance and cached, so the cost is per distinct specifier, not per comparison.

## Alternatives considered

* **Keep the `semver` crate and pre-process npm syntax:** rejected — translating `||`, hyphen
  ranges and bare-version semantics into Cargo's grammar is a parser in itself, and gets
  prerelease matching wrong anyway.
* **Depend on `syncpack-specifier` from crates.io:** rejected — the `Rc` and thread-local design is
  incompatible with ADR 0003, and the changes are not upstream concerns for a single-threaded CLI.
  Vendoring is the smaller fork.
* **Write a new specifier parser:** rejected — syncpack's classification is the specification the
  policies follow; re-deriving it would introduce differences for no gain.
