---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0009", "0031"]
---

# Path Specifiers Belong to local-path-dependency, and Yarn Classic Links Members by Range

## Context and Problem Statement

A dependency can point at a directory on disk: `link:../a`, `file:../a`, `portal:../a` or a bare `../a`.
`local-path-dependency` reports every such specifier. When it names a workspace member, its fix replaces the path with
the specifier that links the member: `workspace:*`, or `*` under npm, which has no `workspace:` protocol.

The version groups of [ADR 0009](0009-version-and-catalog-policies.md) follow syncpack and judge the same entry again.
Next to a member's own version, a bare `../a` is not a semver specifier, so `version-mismatch` reports it as
`DiffersToLocal` and offers to set it to the member's version. A `link:` or `file:` path to another directory gets the
same fix. Next to a registry range, a `file:` path is aligned with that range. `--fix-unsafe` then gets two different
edits for one value (GitHub issue #153), and the `version-mismatch` fix turns a path into a registry range that may
not install at all.

[ADR 0031](0031-core-rule-scoping-peer-overlap-catalog-sources-and-root-discovery-entry-point.md) already leaves path
specifiers to `local-path-dependency` in `workspace-protocol`, so that one dependency yields one finding. It also
makes `local-path-dependency` offer nothing under Yarn Classic, because Yarn Classic has no `workspace:` protocol.
Yarn Classic links a member through a plain range that the member's version satisfies, as npm does, so the npm fix
applies (GitHub issue #160).

How do the version groups treat a path specifier, and what does `local-path-dependency` offer under Yarn Classic?

## Decision Drivers

* One fix per value: two rules never offer different edits for the same specifier
* A fix never turns a path into a registry range the workspace may not publish
* The fix of `local-path-dependency` installs under the detected package manager
* No change for specifiers that are not paths

## Considered Options

* Path specifiers take no part in the version groups
* Path specifiers keep their version-group state, without a fix
* Keep syncpack's behaviour

## Decision Outcome

Chosen option: "Path specifiers take no part in the version groups", because `local-path-dependency` already reports
every path. A second, unfixable finding for the same value adds nothing that rule does not say.

**Version groups.** An instance whose specifier is a path (`link:`, `file:`, `portal:`, `./`, `../` or `/`, the
predicate of `local-path-dependency`) is left out of every version-group policy, as a `catalog:` instance is. It gets
no state. It is never rewritten, never the version other instances align with, and it does not block the alignment
of the others. `banned` groups still report it, because removing a banned dependency rewrites no specifier. A path
that resolves to the member and one that points elsewhere are treated alike: `local-path-dependency` reports both,
and its fix replaces either with the link to the member.

**Yarn Classic.** Under Yarn Classic (1.x), `local-path-dependency` replaces a path to a member with `*`, as under npm.
pnpm, Yarn 2+ and Bun keep `workspace:*`. `workspace-protocol` stays silent under Yarn Classic, as ADR 0031 has it.
The help of the fix names the member it links.

### Consequences

* Good, because each path gets one finding and at most one fix, and a fixed workspace has no `version-mismatch`
  left on the entry.
* Good, because a Yarn Classic workspace gets the same fix as an npm one.
* Bad, because with `local-path-dependency` turned off, a path to the wrong directory is not reported at all.
* Neutral, because the syncpack comparison now differs on paths that syncpack rewrites to a version.

### Confirmation

* `crates/skif-rules/src/policy/visit.rs` tests: path specifiers on a member get no state, and no policy
  (`preferred-semver`, `pinned`, `same-range`, `same-minor`, `snapped-to`) gives a path a state.
* `crates/skif-rules/src/manifest/local_path_dependency.rs` tests: a bare `../a` next to a satisfied range gets one
  fix under npm, Yarn Classic, pnpm, Yarn 2+ and Bun, and the fixed workspace is clean for both rules. The
  replacement follows the package manager, Yarn Classic included.
* `fixtures/policy/version-mismatch/link-specifier-wrong-directory` and `semver-and-file-specifier-mix` expect no
  diagnostic.

## Pros and Cons of the Options

### Path specifiers take no part in the version groups

* Good, because one rule owns each path, as `workspace-protocol` already has it.
* Bad, because the version groups say nothing about a path while `local-path-dependency` is off.

### Path specifiers keep their version-group state, without a fix

* Good, because a wrong path is still reported when `local-path-dependency` is off.
* Bad, because the same value is reported twice, once without a fix, and a bare `../a` to the member needs its own
  path resolution to stay valid.

### Keep syncpack's behaviour

* Good, because the results match syncpack.
* Bad, because `--fix-unsafe` gets conflicting edits, and a path becomes a registry range.

## More Information

* Yarn Classic workspaces: <https://classic.yarnpkg.com/en/docs/workspaces>
* npm workspaces: <https://docs.npmjs.com/cli/using-npm/workspaces>
