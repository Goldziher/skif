# Policy conformance fixtures

Declarative fixtures for the `policy` rules of skif: version groups, semver groups, catalogs, the
workspace protocol, dependency families and the policy conflicts between them
([ADR 0009](../../adrs/0009-version-and-catalog-policies.md)). The fixtures are data only. The conformance
tests in `crates/skif-rules/tests` (`policy_conformance.rs` and `policy_catalog_conformance.rs`) walk them, run the
policy rules and compare the result with `expected.json`.

There are 192 cases in 17 groups. Each case is a minimal monorepo plus the `skif.toml` under
test and the diagnostics skif must report for it, so a failing case points at one behaviour.

## Attribution

The expected results are derived from the behaviour of
[syncpack](https://github.com/JamieMason/syncpack) (MIT, Copyright (c) 2017-2023 Jamie Mason),
read at commit `958d306` (15.3.3): the version-group visitors (`src/version_group/*.rs`), the group
selector (`src/group_selector/`), the specifier ordering (`crates/syncpack-specifier/`) and the
tests next to them. `case.toml` names the syncpack test file and test each case was derived from.
No syncpack code is copied; the workspaces, configuration and expectations are written for skif.
Credit follows [ADR 0015](../../adrs/0015-attribution-and-vendoring.md).

## Layout

```
fixtures/policy/
  README.md
  <group>/<case>/
    case.toml          metadata
    skif.toml          configuration under test
    workspace/         the monorepo (root package.json, pnpm-workspace.yaml or workspaces, members)
    expected.json      diagnostics the policy rules must report
    expected-fixed/    optional: files as they must look after --fix (changed files only)
```

Groups: `version-mismatch`, `pinned`, `banned`, `ignored`, `same-range`, `same-minor`, `snapped-to`,
`semver-groups`, `semver-range-only`, `catalog`, `workspace-protocol`, `dependency-families`, `policy-conflict`,
`selectors`, `invalid-local-version`, `dependency-types`, `catalog-groups`.

| Group | Cases |
| --- | --- |
| `banned` | 4 |
| `catalog` | 32 |
| `catalog-groups` | 6 |
| `dependency-families` | 11 |
| `dependency-types` | 13 |
| `ignored` | 6 |
| `invalid-local-version` | 8 |
| `pinned` | 6 |
| `policy-conflict` | 21 |
| `same-minor` | 5 |
| `same-range` | 8 |
| `selectors` | 14 |
| `semver-groups` | 14 |
| `semver-range-only` | 3 |
| `snapped-to` | 6 |
| `version-mismatch` | 27 |
| `workspace-protocol` | 8 |

## File schemas

### `workspace/`

A complete workspace root. Packages are `packages/<name>/package.json` with `name` equal to the
directory name; the root package is named `monorepo`. Every package has a valid exact `version`
(`1.0.0`, root `0.0.0`) unless the case is about versions, and every manifest is `private`.
`packageManager` in the root selects the package manager (`pnpm`, `npm`, `bun`). pnpm cases carry a
`pnpm-workspace.yaml` (with `catalog:` / `catalogs:` where relevant); Yarn cases keep them in `.yarnrc.yml`; Bun cases keep catalogs in the
root `package.json` (`catalog` / `catalogs`, or under `workspaces` when the case says so); npm cases
use the `workspaces` array. Dependency names are real-looking but nothing is installed.

### `skif.toml`

The configuration under test, in the shape of [ADR 0004](../../adrs/0004-configuration-model.md) and
`skif-config` (kebab-case keys): `[policy]`, `[[version-groups]]` (`policy`, `pin`, `snap-to`,
`prefer`, selectors `dependencies`, `packages`, `dependency-types`, `specifier-types`),
`[[semver-groups]]` (selectors and `range`), `[catalog]`, `[workspace-protocol]`,
`[[dependency-families]]`, `[[ignore]]`, `[workspace]` and `[rules]`. The file lives next to
`workspace/`, not inside it: the harness presents it as `skif.toml` at the workspace root.

### `expected.json`

A JSON array of diagnostics, sorted by `path`, then `rule`, then `dependency`, then `detail`
(byte order; `null` sorts first). Span fields are omitted: line and column are checked separately.

| Key | Meaning |
| --- | --- |
| `rule` | the skif rule ID |
| `severity` | `error`, `warning` or `info`, after `[rules]` overrides (see defaults below) |
| `package` | display name of the package the diagnostic is located in; `null` when the location is not a package manifest (`pnpm-workspace.yaml`, `skif.toml`) |
| `dependency` | the dependency name, or the catalog entry name; `null` when none applies (`UnusedGroup`) |
| `detail` | the syncpack state name from the ADR 0009 table, or `null` for rules without a state |
| `path` | workspace-relative path with `/` separators of the file the diagnostic is located in; `skif.toml` for `UnusedGroup` |
| `fixable` | whether the diagnostic carries a fix |
| `safe` | present and `true` when the diagnostic's fix is `safe`, so `--fix` applies it ([ADR 0051](../../adrs/0051-safe-version-alignment-fixes.md)); absent otherwise |

An empty array means the workspace must be clean.

### `expected-fixed/`

Present when at least one diagnostic is `fixable`. It holds only the files `--fix` changes, at their
workspace-relative paths. `package.json` and `pnpm-workspace.yaml` files are compared as parsed data
(so key order and whitespace are not asserted here; edit-preserving formatting belongs to the
ADR 0008 tests). The fix of a `catalog-missing-entry` also edits the workspace file that holds the
catalog, so that file is listed too.

### `case.toml`

| Key | Meaning |
| --- | --- |
| `description` | what the case demonstrates |
| `source` | `syncpack:<file>::<test>` the case was derived from (paths relative to the syncpack repository, `src/version_group/` abbreviated where unambiguous), or `skif` for cases not taken from a syncpack test |
| `tags` | free-form labels; the ones with meaning to a harness are listed below |
| `valid-states` | optional: syncpack valid states the case exercises without producing a diagnostic (used for the coverage table) |
| `compare` | optional: `"parsed"` (default) compares fixed files as parsed JSON / YAML, `"bytes"` byte for byte |

Tags with meaning: `assumption` and `ambiguous`
(the expectation rests on a reading of the ADRs that the rule implementation must confirm; see
"ADR gaps"), `deviation` (differs from syncpack on purpose; see below), `syncpack-quirk`
(faithful to a surprising syncpack behaviour).

## How a harness should run them

1. Discover every directory below `fixtures/policy/` that contains a `case.toml`.
2. Load `workspace/` into a `MemoryFs` (or copy it to a temp dir) and add `skif.toml` at its root.
3. Build the `Linter` with exactly the twelve policy rules enabled (the rules in the coverage table
   below); every other rule is off, so manifest rules (`local-path-dependency`,
   `root-package-dependencies`, ...) never contribute. `[rules]` in `skif.toml` still applies on top.
4. Run it, drop span and message from each diagnostic, project onto
   `{rule, severity, package, dependency, detail, path, fixable, safe}`, sort as described above and
   compare with `expected.json` for equality.
5. If `expected-fixed/` exists: apply the fixes of all fixable diagnostics and compare each file in
   `expected-fixed/` with the result (parsed JSON / YAML equality, or byte equality when `case.toml`
   sets `compare = "bytes"`); every other file must be unchanged. Then re-run on the fixed workspace
   and expect no diagnostic that is fixable, every fix being idempotent. If `expected-fixed/` is
   absent, no diagnostic may be fixable.
6. Cross-checks worth running once over the whole tree: every `skif.toml` loads and validates, every
   `rule` is registered, and every non-null `detail` appears in the ADR 0009 state table.

## Conventions the fixtures assume

* **Default severities.** The ADRs fix no defaults; the fixtures assume `error` for
  `version-mismatch`, `banned-dependency`, `semver-range-mismatch`, `workspace-protocol`,
  `unresolved-catalog-references`, `invalid-local-version` and
  `unsynced-dependency-family`; `warning` for `catalog-mismatch` (it only reports under `mode = "off"`), `catalog-not-used`, `catalog-missing-entry` (both
  `error` when `[catalog] mode = "enforce"`), `unused-catalog-entries` and `policy-conflict`.
  Severity is per rule, never per state (the emitter overwrites it, ADR 0007).
* **`fixable`** is true for syncpack's Fixable class and false for Unfixable, Conflict and Suspect
  states. `workspace-protocol`, `unsynced-dependency-family` and `unused-catalog-entries` have no
  fix in the ADRs and are not fixable.
* **Built-in implicit semver group.** As in syncpack, the first semver group is a built-in one for
  the workspace packages' own versions (dependency type `local`): they carry no range preference, so
  a catch-all or `$LOCAL` semver group never rewrites a package's own `version`.
* **Dependency types** use syncpack's names `prod`, `dev`, `peer`, `local` and `resolutions`, plus `overrides` and
  `pnpm-overrides` (syncpack: `pnpmOverrides`) and the names of `[[custom-types]]` entries
  ([ADR 0037](../../adrs/0037-override-sections-custom-types-and-ignored-semver-groups.md)).
* **Catalog scope.** With `mode` other than `off`, a dependency that has a catalog entry (or none,
  but is used in at least `min-packages` packages) is owned by the catalog policy and is not also
  reported as a version mismatch. `prefer` and `enforce` report the same instances and differ in
  severity. A configured version group that matches an instance claims it, whatever its policy
  (syncpack's first-match-wins order): the catalog rules skip it, so a `pinned` or `ignored` group can
  keep a package off the catalog (group `catalog-groups`, which runs both families of rules together).
* **Dependency families** are compared within one package: members declared in the same
  `package.json` (any section) must share a version number, ranges ignored; each member below the
  highest is reported.
* **Local packages.** A package without a `version` field is not an invalid local version.

## Deviations from syncpack

Cases marked `deviation` or listed here follow ADR 0009 / skif's model where it differs from
syncpack.

* No `strict` mode: a `workspace:` specifier on a workspace package is always `SatisfiesLocal`
  (syncpack's strict mode reports `DiffersToLocal`). Use `[workspace-protocol]` to require it.
  `version-mismatch/workspace-protocol-satisfies-local`.
* A workspace package with no `version` field is not reported as `InvalidLocalVersion` (syncpack
  reports it with an empty specifier, which would flag every private package). ADR 0009 says "cannot
  be parsed". `invalid-local-version/missing-version-field-not-reported`.
* `unresolved-catalog-references` is reported with `mode = "off"` as well; syncpack only checks
  references inside its `catalog` policy. `catalog/unresolved-reported-with-mode-off`.
* Catalogs are a global `[catalog]` mode rather than a version-group policy. `DiffersToCatalog`
  (`catalog-mismatch`) therefore arises only when a sibling already uses `catalog:`
  (`catalog/off-catalog-mismatch`); with a mode set, plain instances are `NotUsingCatalog`.
* `DiffersToHighestOrLowestSemverMinor` is not covered: skif's `prefer` is valid only on
  `preferred-semver` groups, so a same-minor group cannot name a direction.
* `semverRangeOnly` is the `semver-range-only` policy: versions may differ and only the semver groups' range is
  checked (group `semver-range-only`); syncpack's registry updates inside it are out of scope (ADR 0001). A semver
  group with `ignored = true` covers syncpack's `isIgnored` and `isDisabled`
  (`semver-groups/ignored-group-exempts-dependency`).
* `DiffersToNpmRegistry` is out of scope (ADR 0001); no case.
* Severity is per rule: syncpack's Conflict class (error) and Suspect class (warning) share
  `policy-conflict` and therefore one severity (see "ADR gaps").
* `UnusedGroup` is skif-only (`policy-conflict/unused-group-*`).
* A Bun root with catalogs under both the top level and `workspaces` aborts syncpack; skif reports a
  workspace error (exit 2), which is not a diagnostic, so there is no fixture for it.
* `latest` and dist-tags never rank: they are not an alignment target, and next to a version they are an
  unfixable `NonSemverMismatch` (`version-mismatch/latest-ranks-as-highest`).
* Without `[policy] dependency-types` only `prod` and `dev` dependencies take part, as in sherif; `peer` and
  `optional` are opt-in (`banned/banned-across-sections`, `version-mismatch/peer-dependency-compared` and
  `semver-groups/per-dependency-type-ranges` configure them).
* On a dependency that names a workspace package, a range the package's own version satisfies is valid, as are
  `workspace:` specifiers, `*`/`latest` and a `link:`/`file:` path to the package
  (`version-mismatch/local-package-wins-range-differs`).
* A `catalog:` instance leaves the preferred-semver group; the remaining instances still align.
* Ranges rank by the lowest version they admit, so `^14.1.2` outranks `^14` and the shorthand is the one reported;
  syncpack lets the shorthand win (`version-mismatch/range-ranks-by-lowest-admitted-version`).
* An `npm:` alias of another package is not aligned with the plain dependency of the same name
  (`version-mismatch/npm-alias-not-aligned-with-plain-dependency`).
* One rule owns each instance: while a sibling uses `catalog:`, `catalog-mismatch` reports the plain instances even
  with `mode = "off"` and the version groups leave them alone
  (`version-mismatch/catalog-sibling-owns-instances-mode-off`).
* The override sections (`resolutions`, `overrides`, `pnpm-overrides`) and custom types are governed only when a
  `dependency-types` list names them; syncpack governs every type by default
  (`dependency-types/resolutions-outside-default-scope`, `dependency-types/custom-type-outside-default-scope`).
* `pnpm-overrides` also covers `pnpm.overrides` of `package.json`, which syncpack reads only through a custom type, and
  the dependency of an override entry is the package its selector names (`react@<18`, `parent>react`, `**/react`);
  syncpack takes the key as written (`dependency-types/pnpm-overrides-selector-names`).

## ADR gaps and contradictions found while writing the fixtures

* **Severity defaults** are not specified anywhere. ADR 0009 says an unused group is reported "as a
  warning", ADR 0007 says the emitter overwrites severity per rule, and syncpack treats Conflict
  states as errors. The fixtures set `policy-conflict` to `warning` and cover the override in
  `policy-conflict/unused-group-severity-override`.
* **`DependencyKind` names** are undefined (`prod`/`dev`/`peer` assumed; `optional`, `overrides`,
  `resolutions`, pnpm overrides untested).
* **Group identity in `UnusedGroup`** diagnostics: there is no field that names the group (`label`
  is only available in the message), so the JSON cannot tell two unused groups apart.
* **`detail` for rules without a syncpack state** (`workspace-protocol`,
  `unsynced-dependency-family`, `unused-catalog-entries`) is assumed `null`; ADR 0012 only says the
  values are documented per rule.
* **`unused-catalog-entries`** has no field for the catalog name, which is ambiguous when two
  catalogs define the same dependency.
* **`[workspace-protocol]`** is described as "optionally with a required range" but `skif-config`
  has only `internal` and `allow-local-paths`; no fix target is specified, so these are not fixable.
* **`[[dependency-families]]`**: scope (per package vs workspace-wide), reporting granularity,
  fixability and the names of the built-in families are not specified. Assumed per package, one
  diagnostic per member below the highest, name of the React family `react`.
* **Catalog vs version-group precedence** is open: ADR 0009 expects a `policy-conflict` when "a
  pinned version group and an enforced catalog entry disagree" but names no state for it, so there
  is no fixture. `RefuseToCatalogLocal` is likewise not covered.
* **`min-packages`** gating for `enforce`, and any difference between `prefer` and `enforce` beyond
  severity, are not specified.
* **`$LOCAL`** also selects the packages' own `version` entries (instances of dependency type
  `local`); the built-in no-preference semver group (above) is needed for semver groups to behave
  sensibly and is not mentioned in the ADRs. A banned group using `$LOCAL` reports
  `RefuseToBanLocal` for every package.
* **Snap targets** naming a package that does not exist are not specified;
  `policy-conflict/snap-target-package-unknown` assumes `DependsOnMissingSnapTarget`.
* **Invalid local versions** for a missing `version` field are ambiguous (see deviations).
* Two fix paths are not idempotent by construction (`same-minor-overrides-*-semver-group*`): the
  state table maps them to Fixable but the fix cannot satisfy both groups.

## Coverage

### By rule

| Rule | Cases | Examples |
| --- | --- | --- |
| `version-mismatch` | 49 | `catalog/off-catalog-definition-joins-alignment`, `ignored/ignore-table-suppresses-rule`, `ignored/ignored-dependency-skips-mismatch`, `ignored/ignored-dependency-type` (+45 more) |
| `banned-dependency` | 14 | `banned/banned-across-sections`, `banned/banned-glob-scope`, `banned/banned-scoped-to-package`, `banned/removes-banned-dependency` (+10 more) |
| `semver-range-mismatch` | 15 | `catalog/catalog-definition-semver-group`, `same-range/overlap-but-semver-group-mismatch`, `semver-groups/caret-for-all-dependencies`, `semver-groups/dependency-type-selector` (+11 more) |
| `workspace-protocol` | 4 | `workspace-protocol/dev-dependency-checked`, `workspace-protocol/internal-dependency-without-protocol`, `workspace-protocol/link-path-rejected-by-default`, `workspace-protocol/version-mismatch-and-protocol-both-reported` |
| `catalog-mismatch` | 2 | `catalog/catalogs-resolve-to-different-versions`, `catalog/off-catalog-mismatch` |
| `catalog-version-divergence` | 1 | `catalog/catalogs-resolve-to-different-versions` |
| `catalog-not-used` | 9 | `catalog/ambiguous-dependency-in-no-catalog`, `catalog/ambiguous-dependency-in-two-catalogs`, `catalog/bun-top-level-catalog`, `catalog/bun-workspaces-catalog` (+5 more) |
| `catalog-missing-entry` | 6 | `catalog/bun-missing-entry-creates-catalog`, `catalog/missing-entry-identical-non-semver`, `catalog/missing-entry-mixed-specifiers-unfixable`, `catalog/missing-entry-no-catalog-pnpm` (+2 more) |
| `unused-catalog-entries` | 1 | `catalog/unused-entries-reported` |
| `unresolved-catalog-references` | 4 | `catalog/unresolved-catalog-name`, `catalog/unresolved-dependency-in-other-catalog`, `catalog/unresolved-dependency-not-in-catalog`, `catalog/unresolved-reported-with-mode-off` |
| `policy-conflict` | 20 | `policy-conflict/match-conflicts-with-highest`, `policy-conflict/match-conflicts-with-local`, `policy-conflict/match-conflicts-with-snap-target`, `policy-conflict/mismatch-conflicts-with-highest` (+16 more) |
| `invalid-local-version` | 5 | `invalid-local-version/invalid-local-does-not-block-other-dependencies`, `invalid-local-version/non-semver-package-version`, `invalid-local-version/range-as-package-version`, `invalid-local-version/root-package-invalid-version` (+1 more) |
| `unsynced-dependency-family` | 4 | `dependency-families/builtin-react-family`, `dependency-families/custom-family`, `dependency-families/members-across-sections`, `dependency-families/three-member-trpc-family` |

### By syncpack state

States in the ADR 0009 table. Valid states produce no diagnostic and are listed through the
`valid-states` of the cases that exercise them.

| Class | State (`detail`) | skif rule | Cases |
| --- | --- | --- | --- |
| Valid | `IsCatalog` | none | `catalog/ambiguous-dependency-in-two-catalogs`, `catalog/catalog-definition-semver-group`, `catalog/catalog-references-valid`, `catalog/off-catalog-mismatch` (+2 more) |
| Valid | `IsCatalogDefinition` | none | `catalog/bun-top-level-catalog`, `catalog/bun-workspaces-catalog`, `catalog/catalog-references-valid`, `catalog/min-packages-not-met` (+4 more) |
| Valid | `IsIgnored` | none | `ignored/ignored-dependency-skips-mismatch`, `ignored/ignored-dependency-type`, `ignored/ignored-instances-do-not-influence-highest`, `ignored/ignored-wins-over-semver-group` (+7 more) |
| Valid | `IsLocalAndValid` | none | `invalid-local-version/prerelease-package-version-valid`, `policy-conflict/match-conflicts-with-local`, `policy-conflict/mismatch-conflicts-with-local`, `selectors/not-local-selector` (+6 more) |
| Valid | `IsIdenticalToLocal` | none | `invalid-local-version/prerelease-package-version-valid`, `semver-groups/not-local-selector`, `version-mismatch/local-package-identical-valid`, `version-mismatch/local-package-wins-over-higher` (+1 more) |
| Valid | `SatisfiesLocal` | none | `semver-groups/workspace-protocol-skips-range-check`, `version-mismatch/workspace-protocol-satisfies-local`, `workspace-protocol/external-dependencies-not-checked`, `workspace-protocol/link-path-allowed` (+2 more) |
| Valid | `IsHighestOrLowestSemver` | none | `catalog/exclude-dependency`, `policy-conflict/match-conflicts-with-highest`, `policy-conflict/mismatch-conflicts-with-highest`, `semver-groups/package-selector-aligns-only-members` (+4 more) |
| Valid | `SatisfiesHighestOrLowestSemver` | none | `semver-groups/package-selector-aligns-only-members`, `semver-groups/per-dependency-type-ranges` |
| Valid | `IsNonSemverButIdentical` | none | `same-minor/non-semver-identical-valid`, `version-mismatch/unsupported-specifiers-identical` |
| Valid | `IsIdenticalToPin` | none | `pinned/range-pin`, `pinned/replaces-anything-different`, `pinned/same-number-different-range-no-semver-group`, `pinned/workspace-protocol-pin-valid` (+3 more) |
| Valid | `SatisfiesSameRangeGroup` | none | `same-range/caret-and-tilde-overlap`, `same-range/exact-inside-range`, `same-range/overlap-and-semver-group-match`, `same-range/overlap-but-semver-group-mismatch` (+1 more) |
| Valid | `MatchesSemverGroup` | none | `semver-range-only/non-semver-specifiers-are-left-alone`, `semver-range-only/range-checked-versions-may-differ`, `semver-range-only/versions-differ-without-a-semver-group` |
| Valid | `SatisfiesSameMinorGroup` | none | `policy-conflict/same-minor-overrides-unsafe-caret`, `same-minor/same-minor-different-patches-valid` |
| Valid | `IsIdenticalToSnapTarget` | none | `policy-conflict/match-conflicts-with-snap-target`, `policy-conflict/mismatch-conflicts-with-snap-target`, `policy-conflict/refuse-to-snap-local`, `snapped-to/compatible-semver-group-satisfies-target` (+5 more) |
| Valid | `SatisfiesSnapTarget` | none | `snapped-to/compatible-semver-group-satisfies-target` |
| Fixable | `IsBanned` | `banned-dependency` | `banned/banned-across-sections`, `banned/banned-glob-scope`, `banned/banned-scoped-to-package`, `banned/removes-banned-dependency` (+10 more) |
| Fixable | `DiffersToHighestOrLowestSemver` | `version-mismatch` | `catalog/off-catalog-definition-joins-alignment`, `ignored/ignore-table-suppresses-rule`, `ignored/ignored-dependency-skips-mismatch`, `ignored/ignored-dependency-type` (+23 more) |
| Fixable | `DiffersToHighestOrLowestSemverMinor` | `version-mismatch` | unreachable: no `prefer` for same-minor groups in `skif.toml` (see ADR gaps) |
| Fixable | `DiffersToLocal` | `version-mismatch` | `selectors/not-local-selector`, `version-mismatch/link-specifier-wrong-directory`, `version-mismatch/local-package-wins-over-higher`, `version-mismatch/local-package-wins-range-differs` (+1 more) |
| Fixable | `DiffersToSnapTarget` | `version-mismatch` | `policy-conflict/refuse-to-snap-local`, `snapped-to/fix-applies-semver-group-range`, `snapped-to/snaps-to-target-package`, `snapped-to/target-is-lower-than-others` |
| Fixable | `DiffersToPin` | `version-mismatch` | `pinned/pin-leaves-other-dependencies-to-catch-all`, `pinned/pin-scoped-to-package`, `pinned/range-pin`, `pinned/replaces-anything-different` (+3 more) |
| Fixable | `SemverRangeMismatch` | `semver-range-mismatch` | `catalog/catalog-definition-semver-group`, `same-range/overlap-but-semver-group-mismatch`, `semver-groups/caret-for-all-dependencies`, `semver-groups/dependency-type-selector` (+12 more) |
| Fixable | `PinOverridesSemverRange` | `policy-conflict` | `policy-conflict/pin-overrides-semver-range` |
| Fixable | `PinOverridesSemverRangeMismatch` | `policy-conflict` | `policy-conflict/pin-overrides-semver-range-mismatch` |
| Fixable | `SameMinorOverridesSemverRange` | `policy-conflict` | `policy-conflict/same-minor-overrides-caret-semver-group`, `policy-conflict/same-minor-overrides-unsafe-caret` |
| Fixable | `SameMinorOverridesSemverRangeMismatch` | `policy-conflict` | `policy-conflict/same-minor-overrides-caret-semver-group-mismatch` |
| Fixable | `DiffersToCatalog` | `catalog-mismatch` | `catalog/off-catalog-mismatch` |
| Fixable | `NotUsingCatalog` | `catalog-not-used` | `catalog/bun-top-level-catalog`, `catalog/bun-workspaces-catalog`, `catalog/enforce-not-using-catalog`, `catalog/exclude-dependency` (+3 more) |
| Fixable | `MissingFromCatalog` | `catalog-missing-entry` | `catalog/bun-missing-entry-creates-catalog`, `catalog/missing-entry-identical-non-semver`, `catalog/missing-entry-no-catalog-pnpm`, `catalog/missing-entry-single-catalog` |
| Fixable | `DiffersToNpmRegistry` | none (out of scope) | out of scope (ADR 0001) |
| Unfixable | `NonSemverMismatch` | `version-mismatch` | `same-minor/non-semver-differing-unfixable`, `version-mismatch/unsupported-specifiers-differ` |
| Unfixable | `SameRangeMismatch` | `version-mismatch` | `same-range/different-exact-versions`, `same-range/one-disjoint-range-flags-all`, `same-range/tilde-different-minors` |
| Unfixable | `SameMinorMismatch` | `version-mismatch` | `same-minor/minor-mismatch-unfixable` |
| Unfixable | `SameMinorHasMajorMismatch` | `version-mismatch` | `same-minor/major-mismatch-unfixable` |
| Unfixable | `DependsOnInvalidLocalPackage` | `invalid-local-version` | `invalid-local-version/range-as-package-version`, `invalid-local-version/two-invalid-locals-and-a-dependent` |
| Unfixable | `MissingFromCatalogAndNonSemverMismatch` | `catalog-missing-entry` | `catalog/missing-entry-mixed-specifiers-unfixable` |
| Unfixable | `CannotInferCatalogFile` | `catalog-missing-entry` | `catalog/missing-entry-npm-cannot-infer-file` |
| Unfixable | `NotUsingCatalogAndCatalogUnknown` | `catalog-not-used` | `catalog/ambiguous-dependency-in-no-catalog`, `catalog/ambiguous-dependency-in-two-catalogs` |
| Conflict | `MatchConflictsWithHighestOrLowestSemver` | `policy-conflict` | `policy-conflict/match-conflicts-with-highest` |
| Conflict | `MismatchConflictsWithHighestOrLowestSemver` | `policy-conflict` | `policy-conflict/mismatch-conflicts-with-highest` |
| Conflict | `MatchConflictsWithSnapTarget` | `policy-conflict` | `policy-conflict/match-conflicts-with-snap-target` |
| Conflict | `MismatchConflictsWithSnapTarget` | `policy-conflict` | `policy-conflict/mismatch-conflicts-with-snap-target` |
| Conflict | `MatchConflictsWithLocal` | `policy-conflict` | `policy-conflict/match-conflicts-with-local` |
| Conflict | `MismatchConflictsWithLocal` | `policy-conflict` | `policy-conflict/mismatch-conflicts-with-local` |
| Suspect | `InvalidLocalVersion` | `invalid-local-version` | `invalid-local-version/invalid-local-does-not-block-other-dependencies`, `invalid-local-version/non-semver-package-version`, `invalid-local-version/range-as-package-version`, `invalid-local-version/root-package-invalid-version` (+1 more) |
| Suspect | `DependsOnMissingCatalogDefinition` | `unresolved-catalog-references` | `catalog/unresolved-catalog-name`, `catalog/unresolved-dependency-in-other-catalog`, `catalog/unresolved-dependency-not-in-catalog`, `catalog/unresolved-reported-with-mode-off` |
| Suspect | `DiffersToOtherCatalog` | `catalog-version-divergence` | `catalog/catalogs-resolve-to-different-versions` |
| Suspect | `RefuseToBanLocal` | `policy-conflict` | `policy-conflict/refuse-to-ban-local` |
| Suspect | `RefuseToPinLocal` | `policy-conflict` | `policy-conflict/refuse-to-pin-local` |
| Suspect | `RefuseToSnapLocal` | `policy-conflict` | `policy-conflict/refuse-to-snap-local` |
| Suspect | `RefuseToCatalogLocal` | `policy-conflict` | not covered: semantics under a global `[catalog] mode` are undefined (see ADR gaps) |
| Suspect | `DependsOnMissingSnapTarget` | `policy-conflict` | `policy-conflict/snap-target-does-not-declare-dependency`, `policy-conflict/snap-target-package-unknown` |
| skif | `UnusedGroup` | `policy-conflict` | `policy-conflict/unused-group-dependency-type-absent`, `policy-conflict/unused-group-matches-nothing`, `policy-conflict/unused-group-severity-override`, `policy-conflict/unused-group-shadowed-by-earlier-group` |
