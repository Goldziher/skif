<!-- Generated from the rule metadata; run `SKIF_UPDATE_DOCS=1 cargo test -p skif-rules --test docs` to update. -->

# Rules

| Rule | Category | Default severity | Fixable |
| --- | --- | --- | --- |
| [`catalog-mismatch`](#catalog-mismatch) | catalog | error | sometimes |
| [`catalog-missing-entry`](#catalog-missing-entry) | catalog | warning | sometimes |
| [`catalog-not-used`](#catalog-not-used) | catalog | warning | sometimes |
| [`unresolved-catalog-references`](#unresolved-catalog-references) | catalog | error | none |
| [`unused-catalog-entries`](#unused-catalog-entries) | catalog | warning | none |
| [`empty-dependencies`](#empty-dependencies) | manifest | error | safe |
| [`incompatible-peer-dependencies`](#incompatible-peer-dependencies) | manifest | warning | none |
| [`local-path-dependency`](#local-path-dependency) | manifest | error | sometimes |
| [`member-toolchain-fields`](#member-toolchain-fields) | manifest | warning | unsafe |
| [`root-package-dependencies`](#root-package-dependencies) | manifest | warning | unsafe |
| [`root-package-manager-field`](#root-package-manager-field) | manifest | error | none |
| [`root-package-private-field`](#root-package-private-field) | manifest | error | safe |
| [`types-in-dependencies`](#types-in-dependencies) | manifest | error | unsafe |
| [`unsorted-dependencies`](#unsorted-dependencies) | manifest | error | safe |
| [`banned-dependency`](#banned-dependency) | policy | error | unsafe |
| [`invalid-local-version`](#invalid-local-version) | policy | error | none |
| [`policy-conflict`](#policy-conflict) | policy | warning | sometimes |
| [`semver-range-mismatch`](#semver-range-mismatch) | policy | error | unsafe |
| [`unsynced-dependency-family`](#unsynced-dependency-family) | policy | error | none |
| [`version-mismatch`](#version-mismatch) | policy | error | sometimes |
| [`workspace-protocol`](#workspace-protocol) | policy | error | none |
| [`missing-package-manifest`](#missing-package-manifest) | workspace | warning | unsafe |
| [`unmatched-workspace-patterns`](#unmatched-workspace-patterns) | workspace | warning | unsafe |

## catalog-mismatch

A dependency is declared with a version while other packages take it from a catalog.

- Category: catalog
- Default severity: error
- Fixable: sometimes

## catalog-missing-entry

A dependency that should come from a catalog has no catalog entry.

- Category: catalog
- Default severity: warning
- Fixable: sometimes

## catalog-not-used

A dependency that has a catalog entry is declared with a version instead of `catalog:`.

- Category: catalog
- Default severity: warning
- Fixable: sometimes

## unresolved-catalog-references

A `catalog:` specifier names a catalog or an entry that does not exist.

- Category: catalog
- Default severity: error
- Fixable: none

## unused-catalog-entries

A catalog entry that no manifest references.

- Category: catalog
- Default severity: warning
- Fixable: none

## empty-dependencies

A dependency section (`dependencies`, `devDependencies`, `peerDependencies`, `optionalDependencies`) is an empty object.

- Category: manifest
- Default severity: error
- Fixable: safe

## incompatible-peer-dependencies

The version a package installs in `devDependencies` does not satisfy the range it declares in `peerDependencies`.

- Category: manifest
- Default severity: warning
- Fixable: none

## local-path-dependency

A dependency points at a path on disk (`link:`, `file:`, `portal:` or a relative path) instead of a registry or workspace version.

- Category: manifest
- Default severity: error
- Fixable: sometimes
- sherif name: `local-path-dependencies`

## member-toolchain-fields

Member packages should not declare `packageManager` or `engines`; the root package owns the toolchain.

- Category: manifest
- Default severity: warning
- Fixable: unsafe

## root-package-dependencies

The root package should only have `devDependencies`; runtime dependencies belong to the packages that use them.

- Category: manifest
- Default severity: warning
- Fixable: unsafe

## root-package-manager-field

The root `package.json` must declare the package manager in `packageManager` or `devEngines.packageManager`.

- Category: manifest
- Default severity: error
- Fixable: none

## root-package-private-field

The root `package.json` must set `"private": true` so it cannot be published by accident.

- Category: manifest
- Default severity: error
- Fixable: safe

## types-in-dependencies

A private package lists `@types/*` packages in `dependencies`; type packages belong in `devDependencies`.

- Category: manifest
- Default severity: error
- Fixable: unsafe

## unsorted-dependencies

The entries of a dependency section are not in alphabetical (byte) order.

- Category: manifest
- Default severity: error
- Fixable: safe
- sherif name: `unordered-dependencies`

## banned-dependency

A dependency is declared although a banned version group forbids it.

- Category: policy
- Default severity: error
- Fixable: unsafe

## invalid-local-version

A workspace package has a `version` that is not an exact semver version, so dependents cannot align with it.

- Category: policy
- Default severity: error
- Fixable: none

## policy-conflict

Two policies require incompatible outcomes for one dependency, or a group cannot be applied as configured.

- Category: policy
- Default severity: warning
- Fixable: sometimes

## semver-range-mismatch

A dependency uses a semver range other than the one its semver group requires.

- Category: policy
- Default severity: error
- Fixable: unsafe

## unsynced-dependency-family

Members of a dependency family declared in one package are on different versions.

- Category: policy
- Default severity: error
- Fixable: none
- sherif name: `unsync-similar-dependencies`

## version-mismatch

A dependency instance differs from the version its version group requires (highest, lowest, pinned, local or snapped).

- Category: policy
- Default severity: error
- Fixable: sometimes
- sherif name: `multiple-dependency-versions`

## workspace-protocol

A dependency on a workspace package does not use the `workspace:` protocol.

- Category: policy
- Default severity: error
- Fixable: none

## missing-package-manifest

A directory matched by the workspace patterns has no `package.json`.

- Category: workspace
- Default severity: warning
- Fixable: unsafe
- sherif name: `packages-without-package-json`

## unmatched-workspace-patterns

A workspace pattern does not match any directory.

- Category: workspace
- Default severity: warning
- Fixable: unsafe
- sherif name: `non-existant-packages`

## Unimplemented states

These policy states of ADR 0009 are not emitted yet.

- `DiffersToHighestOrLowestSemverMinor`: a `same-minor` group cannot name a direction, see ADR 0009
- `DiffersToNpmRegistry`: registry updates are out of scope, see ADR 0001
- `RefuseToCatalogLocal`: a workspace package is never moved into a catalog
