---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0009", "0031"]
---

# Path Specifiers Belong to local-path-dependency, and Yarn Classic Links Members by Range

## Context and Problem Statement

A dependency can point at a directory on disk: `link:../a`, `file:../a`, `portal:../a`, a bare `../a` or a
home-relative `~/a`. `local-path-dependency` reports every such specifier in the four dependency sections. When it names a workspace member, its fix replaces the path with
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
* A path to the wrong directory is reported by some rule, whichever rules are on

## Considered Options

* Path specifiers take no part in the version groups
* Path specifiers keep their version-group state, without a fix
* Leave out the paths `local-path-dependency` reports; the others keep their state, without a fix
* Keep syncpack's behaviour

## Decision Outcome

Chosen option: "Leave out the paths `local-path-dependency` reports; the others keep their state, without a fix",
because one rule owns each path while that rule reports it, and a path is still reported when it does not.

**Version groups.** A path specifier (`link:`, `file:`, `portal:`, `./`, `../`, `/` or `~/`, the predicate of
`local-path-dependency`) is never rewritten and never the version other instances align with, nor a snap target.

* In a dependency section, while `local-path-dependency` is on, the instance is left out of every version-group
  policy, as a `catalog:` instance is. It gets no state and does not block the alignment of the others, as
  `workspace-protocol` already leaves such a path to that rule.
* With `local-path-dependency` off, and in an override section or a custom type, which that rule does not read, the
  instance keeps its version-group state, but the state carries no fix. Next to a workspace package's own version, a
  path that resolves to the package's directory is valid, and a path to another directory is `DiffersToLocal`.
* `banned` groups report a path in every case, because removing a banned dependency rewrites no specifier.

**local-path-dependency.** A path to a workspace member gets the fix that links the member, whether or not the path
resolves to the member's directory. When it resolves elsewhere, or outside the workspace, the help names both
directories: the fix stays unsafe, because linking the member installs other code than the path.

**Yarn Classic.** Under Yarn Classic (1.x), `local-path-dependency` replaces a path to a member with `*`, as under npm.
Yarn Classic matches a member's version against the range without prereleases, so a member with a prerelease version
is linked by its exact version. npm accepts `*` for any version of a member. pnpm, Yarn 2+ and Bun keep
`workspace:*`. `workspace-protocol` stays silent under Yarn Classic, as ADR 0031 has it. The help of the fix names
the member it links.

### Consequences

* Good, because each path gets one finding and at most one fix while `local-path-dependency` is on, and a fixed
  workspace has no `version-mismatch` left on the entry.
* Good, because a path to the wrong directory is still reported, without a fix, while `local-path-dependency` is off,
  and a path in an override section is reported by the version groups.
* Good, because a Yarn Classic workspace gets the same fix as an npm one.
* Bad, because the version groups judge a path differently depending on whether another rule is on.
* Neutral, because the syncpack comparison now differs on paths that syncpack rewrites to a version.

### Confirmation

* `crates/skif-rules/src/policy/visit.rs` tests: with `local-path-dependency` on, a path in a dependency section gets
  no state under any policy (`preferred-semver`, `pinned`, `same-range`, `same-minor`, `snapped-to`) and the other
  instances keep theirs; with it off, the path keeps its state with no target, a path to the local package's
  directory is valid and one to another directory is `DiffersToLocal`. A path is never the snap target, `banned`
  groups report it, and a path in an override section keeps its state either way.
* `crates/skif-rules/src/manifest/local_path_dependency.rs` tests: a bare `../a` next to a satisfied range gets one
  fix under npm, Yarn Classic, pnpm, Yarn 2+ and Bun, and the fixed workspace is clean for both rules. The
  replacement follows the package manager, Yarn Classic included, and a prerelease member is linked by its exact
  version under Yarn Classic. A path to another directory keeps an unsafe fix and its help names both directories.
* `fixtures/policy/version-mismatch/link-specifier-wrong-directory` and `semver-and-file-specifier-mix` expect no
  diagnostic.

## Pros and Cons of the Options

### Path specifiers take no part in the version groups

* Good, because one rule owns each path, as `workspace-protocol` already has it.
* Bad, because the version groups say nothing about a path while `local-path-dependency` is off, and nothing reports
  a path in an override section.

### Leave out the paths `local-path-dependency` reports; the others keep their state, without a fix

* Good, because one rule owns each path that rule reports, and every other path is still judged.
* Bad, because the version groups need the path resolution of `local-path-dependency` to accept a path to the
  package itself.

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
* npm accepts `*` for any version in its dependency check (`depValid`):
  <https://github.com/npm/cli/blob/latest/workspaces/arborist/lib/dep-valid.js>
