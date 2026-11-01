---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Version and Catalog Policies: syncpack Semantics, Few Stable Rule IDs

## Context and Problem Statement

sherif enforces one version policy, hard-coded: `multiple-dependency-versions` reports a dependency
declared with more than one version across the workspace, and `unsync-similar-dependencies` reports
related packages from a hard-coded list (tRPC, React, Next.js, Storybook, Prisma, typescript-eslint,
Playwright and others) on different versions. Neither can be scoped, pinned, or relaxed for one
package, and the family list changes only with a release.

syncpack solved the general problem and its model is well understood by its users:

* **Version groups** partition dependency instances by selectors (dependency name, dependency type,
  package, specifier type) and give each group a policy: `pinned`, `banned`, `ignored`, `same-range`,
  `same-minor`, `snapped-to`, or the default highest/lowest-semver alignment. **The first group whose
  selectors match an instance owns it.**
* **Semver groups** independently decide the range type (`^`, `~`, exact, …) an instance should use.
* Every instance ends in a **state** — valid, or one of many specific fixable, unfixable or
  suspect states — that explains exactly why.

pnpm, bun and yarn catalogs add a second source of truth for versions, and the `workspace:` protocol
a third for internal packages. syncpack's state machine is the most complete specification of how
these interact.

## Decision Drivers

* sherif's two hard-coded version rules cannot be scoped, pinned or relaxed
* syncpack's model of version groups, semver groups and instance states is well understood by its users
* Catalogs and the `workspace:` protocol add further sources of truth for versions
* Rule IDs are public API, so the set must stay small and stable

## Considered Options

* Re-implement syncpack's group semantics in Rust, mapped onto a few stable rule IDs
* One rule ID per syncpack state
* Keep sherif's fixed rules and add options
* Run syncpack's Rust core as a library

## Decision Outcome

Chosen option: "Re-implement syncpack's group semantics in Rust, mapped onto a few stable rule IDs", because syncpack users find the model they know, sherif's default check survives as a configurable catch-all, and a handful of rule IDs keeps configuration usable while `detail` keeps full precision.

`skif-rules/policy` **re-implements syncpack's semantics** in Rust on skif's own model. syncpack is
the specification, not a dependency: behaviour is taken from its documentation and observed
results, and the table tests derived from it credit syncpack as their source ([ADR 0015](0015-attribution-and-vendoring.md)). The one
vendored part is syncpack's `pattern_matcher` for selector globs, alongside the specifier model
([ADR 0006](0006-specifier-and-semver-model.md)).

### Version groups

`[[version-groups]]` in `skif.toml` ([ADR 0004](0004-configuration-model.md)) are evaluated **in file order, first match wins**,
over the `DependencyInstance`s from discovery ([ADR 0005](0005-workspace-discovery.md)). Selectors: `dependencies`,
`dependency-types`, `packages`, `specifier-types`, with negation; `$LOCAL` selects dependencies that
are themselves workspace packages. Policies: `pinned`, `banned`, `ignored`, `same-range`,
`same-minor`, `snapped-to`, `preferred-semver` (with `[policy] prefer = "highest" | "lowest"`).

After the configured groups, skif appends a **catch-all `preferred-semver` group** matching every
remaining instance. That catch-all is sherif's `multiple-dependency-versions` expressed as a policy,
and it reports under `version-mismatch`. A workspace with no `skif.toml` therefore still gets
sherif's default behaviour; a workspace with groups gets it only for what its groups do not claim.

### Semver groups, catalogs, workspace protocol, families

* `[[semver-groups]]` — same selectors, first match wins, policy is a range type; reported as
  `semver-range-mismatch`.
* `[catalog]` — `mode = "off" | "prefer" | "enforce"`. `prefer` suggests `catalog:` for a
  dependency used in at least `min-packages` packages that has a catalog entry; `enforce` requires
  it. Reported as `catalog-not-used`, `catalog-mismatch` (instance differs from the catalog entry),
  `catalog-missing-entry` (a dependency that should be catalogued has no entry),
  `unresolved-catalog-references` (`catalog:name` with no such catalog or entry) and, with
  `report-unused-entries`, `unused-catalog-entries`.
* `[workspace-protocol]` — internal dependencies must use `workspace:`, optionally with a required
  range; reported as `workspace-protocol`. `invalid-local-version` reports a workspace package whose
  own `version` cannot be parsed, which breaks `$LOCAL` alignment.
* `[[dependency-families]]` — named sets of packages released together that must share a version;
  reported as `unsynced-dependency-family`. sherif's hard-coded list becomes the built-in default
  families, and configuration can add to or disable them.
* `banned` version groups report `banned-dependency`.

### States map to a few rule IDs

syncpack distinguishes dozens of instance states. skif maps them onto the **small, stable set of
rule IDs above** ([ADR 0007](0007-rule-and-diagnostic-model.md)) and carries the exact state in `Diagnostic.detail`:

```json
{ "rule": "version-mismatch",
  "detail": { "state": "DiffersToHighestOrLowestSemver", "group": 3, "expected": "^18.3.1" } }
```

Users configure and suppress by rule ID; tooling that needs syncpack-level precision reads `detail`.
When two policies require incompatible outcomes for one instance — a pinned version group and an
enforced catalog entry that disagree — skif reports `policy-conflict` once instead of two diagnostics
whose fixes would undo each other.

Fixes are `Unsafe` ([ADR 0008](0008-fix-and-edit-model.md)): they change what gets installed.

### Consequences

* Good, because syncpack users find the model they know, configured in TOML; `mise run diff:syncpack` compares
  skif's states with syncpack's on the corpus and turns parity into a measured number.
* Good, because sherif's default check survives as a configurable catch-all rather than a special case.
* Good, because A handful of rule IDs keeps `[rules]` and `[[ignore]]` usable while `detail` keeps full precision.
* Bad, because re-implementing a state machine from a specification means matching its edge cases one by one.
  The table tests are the defence and must cover every state skif claims to emit.
* Bad, because first-match-wins is order-sensitive; a group placed too early silently claims instances. `skif`
  reports a configured group that matches nothing as a warning.
* Bad, because state names in `detail` follow syncpack's and become part of the JSON contract ([ADR 0012](0012-reporting.md)), so a
  syncpack rename does not propagate automatically.

### Confirmation

* Table tests derived from syncpack cover every state skif claims to emit and credit syncpack as their source ([ADR 0015](0015-attribution-and-vendoring.md)).
* `mise run diff:syncpack` compares skif's states with syncpack's on the corpus and turns parity into a measured number.
* skif reports a configured group that matches nothing as a warning.

## Pros and Cons of the Options

### Re-implement syncpack's group semantics in Rust, mapped onto a few stable rule IDs

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### One rule ID per syncpack state

* Bad, because it creates dozens of IDs, most of which nobody would configure individually, and each a permanent public name.

### Keep sherif's fixed rules and add options

* Bad, because every option would be a partial re-invention of version groups.

### Run syncpack's Rust core as a library

* Bad, because it is a CLI crate with its own config loading, output and process model, and the parts skif needs are exactly the ones it vendors.

## More Information

* Related decisions: [ADR 0004](0004-configuration-model.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0012](0012-reporting.md), [ADR 0015](0015-attribution-and-vendoring.md)
* Upstream: [syncpack](https://github.com/JamieMason/syncpack)
* See the [ADR index](index.md).
