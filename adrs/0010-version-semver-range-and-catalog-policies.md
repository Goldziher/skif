---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Version, Semver-Range and Catalog Policies: syncpack Semantics on a Few Stable Rule IDs

## Context and Problem Statement

sherif enforces one hard-coded version policy: a dependency declared with more than one version across the
workspace, and related packages from a fixed list on different versions. Neither can be scoped, pinned or relaxed.
syncpack solved the general problem with a model its users know: version groups that partition dependency instances by
selectors and give each group a policy, the first matching group owning an instance; semver groups that decide the
range type independently; and a state per instance that says exactly why it is valid or not. pnpm, Yarn and Bun
catalogs add a second source of truth for versions, the `workspace:` protocol a third for internal packages, and
override sections and custom fields such as `engines.node` and `packageManager` hold versions too.

How does skif express version, range and catalog policies so that a syncpack user finds the model they know, a
workspace without configuration keeps sherif's default check, and configuration stays usable?

## Decision Drivers

* syncpack users find the semantics they know, configured in `skif.toml`
* A workspace without configuration still gets sherif's default check
* A few stable rule IDs keep `[rules]` and `[[ignore]]` usable, while the exact state stays available to tooling
* One rule owns each finding: two rules never report the same instance with fixes that undo each other
* Ranges that state a compatibility window, such as peer ranges, are not aligned unless asked
* A fix never writes into a file the active package manager does not read

## Considered Options

* Re-implement syncpack's group semantics in Rust, mapped onto a few stable rule IDs with the state in `detail`
* One rule ID per syncpack state
* Vendor or call syncpack
* Keep sherif's two hard-coded checks

## Decision Outcome

Chosen option: "Re-implement syncpack's group semantics in Rust, mapped onto a few stable rule IDs with the state in
`detail`", because syncpack users find the model they know, sherif's check survives as a configurable catch-all, and a
handful of rule IDs keeps configuration usable while `detail` keeps full precision.

### syncpack as the specification

* `skif-rules/policy` re-implements syncpack's semantics on skif's own model; syncpack is the specification, not a
  dependency, and the table tests derived from it credit it ([ADR 0033](0033-attribution-and-vendoring.md)). Only the
  specifier model is vendored ([ADR 0007](0007-specifier-and-semver-model.md)). Selector matching is the compiled
  `Selector` of `skif-config` ([ADR 0004](0004-configuration-model-and-layering.md)), shared with `[[ignore]]`,
  `ignore-packages` and the JS ignore lists.
* Every state syncpack can assign maps to exactly one rule or to none, and the state name is the diagnostic's `detail`,
  part of the JSON contract ([ADR 0022](0022-reporting-and-output-formats.md)). The mapping lives with the code and is
  checked by a test, not restated here. Where skif's reading departs from syncpack's, a state follows skif's reading:
  a dependency that none of several catalogs defines is `MissingFromCatalogAndCatalogUnknown`
  (`catalog-missing-entry`), and `NotUsingCatalogAndCatalogUnknown` is left to a dependency several catalogs define;
  `DiffersToNpmRegistry` maps to no rule and is reported by `skif update` ([ADR 0026](0026-skif-update.md)).
* States skif adds are `UnusedGroup`, `InvalidPin`, `DiffersToOtherCatalog` and `IsIdenticalToCatalog`.
* Deliberate differences from syncpack's results: a range the local version satisfies is valid next to it; a package
  without a valid `version` is no local instance; `latest` and `*` never become the alignment target and are reported
  unfixable on the floating side; ranges rank by the lowest version they admit, so a shorthand never outranks a longer
  range of the same floor; an `npm:` alias is a dependency of its own; packages are read with the package manager's
  workspace semantics ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)). The comparison harness
  classifies these ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)).

### Scope

* Without `[policy] dependency-types`, the version, semver-range and catalog policies cover `dependencies` and
  `devDependencies`, plus each package's own version (`local`). Peer and optional ranges state a compatibility window,
  so aligning them is opt-in, and override sections and custom types are opt-in too: a configured group that names the
  type governs it, and the catch-all governs it only when `[policy] dependency-types` names it.
* `dependency-types` tokens are `prod`, `dev`, `peer`, `optional`, `local`, `resolutions`, `overrides`,
  `pnpm-overrides` and the name of any `[[custom-types]]` entry, each negatable with `!`; syncpack's spellings are
  accepted. An unknown token fails the configuration load, with the closest name suggested.
* The own version follows the list like any type: an empty or absent list includes it, a list of plain types must name
  `local`, and a list of negations must not negate it. `--dependency-types` replaces the list for a run.
* Override sections are read from every active package: `resolutions`, npm `overrides` (nested objects, where a `.`
  entry belongs to its parent) and `pnpm-overrides` (`pnpm.overrides` of a manifest and `overrides` of
  `pnpm-workspace.yaml`). Only string values are instances. `[policy] override-reading` decides the dependency of an
  entry: `skif` (default) takes the package its selector replaces, so `react@<18`, `parent>react` and `**/react` are
  instances of `react`; `syncpack` takes the key as written, skips nested entries and skips `pnpm.overrides` of
  manifests.
* `[[custom-types]]` entries have `name`, `path` (dotted keys in `package.json`) and `strategy`: `version`,
  `name@version`, `versions-by-name`, or `name~version` with a `name-path`. A name is unique and not a built-in token.
  Entries merge by name across layered files, `reset = ["custom-types"]` discards inherited ones, and an override table
  cannot carry them. A fix rewrites the string in place and keeps a `name@` prefix.

### Version groups

* `[[version-groups]]` are evaluated in file order, first match wins, over the parsed instances of discovery.
  Selectors are `dependencies`, `dependency-types`, `packages` and `specifier-types`, each with `!` negation;
  `$LOCAL` selects dependencies that are workspace packages. Policies are `pinned`, `banned`, `ignored`,
  `same-range`, `same-minor`, `snapped-to`, `preferred-semver` (with `[policy] prefer = "highest" | "lowest"`) and
  `semver-range-only` (versions may differ, ranges must match the semver group).
* After the configured groups a catch-all `preferred-semver` group takes every remaining instance in scope and reports
  `version-mismatch`: sherif's check, expressed as a policy.
* A configured group that matches nothing is `policy-conflict` with `detail` `UnusedGroup`, so an ordering mistake is
  visible; a group the instance filters leave empty is not reported. When two policies require incompatible outcomes
  for one instance, one `policy-conflict` is reported; where syncpack has a winner (a pin beats a semver group), the fix
  follows the winner.
* The first matching configured group claims an instance, whatever its policy. The catalog rules do not see a claimed
  instance: it is neither a candidate nor a sibling, does not count towards `min-packages`, does not decide a new
  entry's version, and is not reported by `unresolved-catalog-references`. `unused-catalog-entries` still counts a
  claimed `catalog:` reference as a use. A group without a `dependencies` selector therefore exempts its whole package
  from the catalog rules. Semver groups claim nothing.
* `snapped-to` looks its target up among all instances of the dependency in the workspace, not only those the group
  selected; within the snap-to package, production beats development beats any other section, and a peer range is
  never the target.
* Path specifiers (`link:`, `file:`, `portal:`, `./`, `../`, `/`, `~/`) are never rewritten and never an alignment or
  snap target. While `local-path-dependency` is on, such an instance in a dependency section takes part in no
  version-group policy and that rule owns it; with the rule off, and in override sections and custom types, it keeps
  its state without a fix. `banned` reports a path in every case.
* `[[version-groups]]` and `[[dependency-families]]` also take a lockfile `scope`, judged on the lockfile
  ([ADR 0011](0011-lockfile-and-toolchain-rules.md)).

### Semver groups, the workspace protocol and families

* `[[semver-groups]]` use the same selectors, first match wins, and each entry has exactly one of `range` and
  `ignored = true`. An ignored group matches like any other and stops later groups. Findings are
  `semver-range-mismatch`.
* `workspace-protocol` requires `workspace:` for internal dependencies, optionally with a range. It is silent when the
  package manager cannot install the protocol (npm, Yarn Classic) and for peer dependencies, and active when the
  manager is unknown and `internal = true` asks for it. While `local-path-dependency` is on, paths are left to it.
  `invalid-local-version` reports a workspace package whose own version does not parse.
* `local-path-dependency` replaces a path to a member with `*` under npm and Yarn Classic (the exact version for a
  member with a prerelease under Yarn Classic) and with `workspace:*` under pnpm, Yarn Berry and Bun. The fix is
  unsafe, and its help names both directories when the path resolves elsewhere.
* `incompatible-peer-dependencies` reports a development range when no version it admits satisfies the peer range,
  judged by range overlap.
* `[[dependency-families]]` are sets of packages released together that must share a version, reported as
  `unsynced-dependency-family`. The built-in families cover sherif's list and add more; configuration adds families
  and replaces a built-in one by name.

### Catalogs

* `[catalog] mode` is `off` (default), `prefer` or `enforce`, with `min-packages`, `exclude` and
  `report-unused-entries`. The rules are `catalog-not-used`, `catalog-mismatch`, `catalog-missing-entry`,
  `unresolved-catalog-references` (also in override sections) and `unused-catalog-entries`.
* Each catalog source is read for the manager that uses it: `pnpm-workspace.yaml` under pnpm or an unknown manager,
  `.yarnrc.yml` under Yarn or an unknown manager, and the root `package.json` always. A `.yarnrc.yml` that does not
  parse is a manifest error, and the workspace is discovered without its catalogs.
* `catalog-mismatch` reports only in `mode = "off"`, so its default severity is a warning, and its message names the
  version of the catalog the siblings use.
* `catalog-version-divergence` (warning, no fix) reports references that resolve through different catalogs to
  different versions, in every mode. It compares versions as written, leaves peers out and leaves claimed instances to
  their group. Which catalog is right is the owner's decision.

### Listing and filtering

* `skif list` and `skif json` show every instance the version groups and the catalog govern, valid or not, ordered by
  group, dependency and place, with the state the policy and catalog rules give it; `json` prints syncpack's keys plus
  skif's own. `list --show` defaults to `hints,statuses`. Both exit 1 when an instance is reported at `error`.
* `--dependency`, `--dependency-types`, `--specifier-types` and `--source` drop the instances they do not select before
  groups are assigned, as syncpack does. They reach only the version-group and semver-group rules and the listings; the
  catalog, protocol, family and manifest rules see the whole workspace. A non-empty `--dependency-types` is also the
  run's default scope. The filter is a field of `Config` that no file can set.

### Not decided

Per-group and per-state severity, syncpack's `dependencyGroups`, a catalog policy scoped by selectors, the custom type
`source`, catalog entries as instances and `.x` ranges are not supported, and no design for them is chosen.

### Consequences

* Good, because syncpack users find the model they know, and the listing cannot drift from the rules because it comes
  from the same analysis.
* Good, because a workspace with no configuration gets sherif's check, and a workspace with groups gets it for what its
  groups do not claim.
* Good, because a few rule IDs keep configuration usable while `detail` carries syncpack's precision.
* Good, because `ignored`, `pinned` and `banned` groups behave the same with and without a catalog, and a path or a
  claimed instance has one owner.
* Bad, because re-implementing a state machine from a specification means matching its edge cases one by one.
* Bad, because first match wins is order-sensitive; the `UnusedGroup` finding makes the common mistake visible.
* Bad, because several states share a rule ID, so silencing one state silences its siblings unless `[[ignore]]`
  selectors narrow it.
* Bad, because the default scope and the override reading differ from syncpack, so a migrated configuration has to list
  `dependency-types` ([ADR 0025](0025-skif-migrate.md)), and the filters do not reach the catalog rules.

### Confirmation

* `crates/skif-rules/src/policy/groups.rs`: `the_first_matching_group_owns_the_instance`,
  `the_catch_all_governs_the_sections_the_policy_covers` and `unused_groups_are_visible_in_the_assignment`.
* `crates/skif-rules/src/policy/visit.rs`: `a_pin_beats_a_semver_group` and
  `a_policy_scope_that_does_not_select_local_leaves_the_own_versions_out`; `crates/skif-rules/src/policy/inspect.rs`:
  `every_governed_instance_is_listed_with_its_group_and_state`.
* `crates/skif-rules/tests/policy_catalog_conformance.rs`:
  `catalog_rules_leave_the_instances_of_version_groups_to_their_group` and
  `catalog_workspace_protocol_and_family_fixtures_conform`.
* `crates/skif/tests/policy_conformance.rs`: `policy_fixtures_conform_through_the_facade`,
  `catalog_fixtures_conform_through_the_facade` and `catalog_and_version_group_fixtures_conform_through_the_facade`
  run the conformance fixtures under `fixtures/policy`.

## Pros and Cons of the Options

### Re-implement syncpack's group semantics in Rust, mapped onto a few stable rule IDs with the state in `detail`

* Good, because users get syncpack's model and configuration stays small.
* Bad, because syncpack's edge cases are rediscovered rather than inherited.

### One rule ID per syncpack state

* Good, because each state could be configured on its own.
* Bad, because dozens of rule IDs make `[rules]` and `[[ignore]]` unusable and tie rule IDs to syncpack's renames.

### Vendor or call syncpack

* Good, because results would match syncpack exactly.
* Bad, because it brings a Node runtime or a large vendored tree, and skif's own model of workspaces and specifiers
  would be duplicated.

### Keep sherif's two hard-coded checks

* Good, because nothing new is built.
* Bad, because nothing can be pinned, scoped, banned or relaxed.
