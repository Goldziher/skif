<!-- Generated from the rule metadata; run `SKIF_UPDATE_DOCS=1 cargo test -p skif --features js-analysis --test docs` to update. -->

# Rules

| Rule | Category | Default severity | Fixable |
| --- | --- | --- | --- |
| [`catalog-mismatch`](#catalog-mismatch) | catalog | warning | sometimes |
| [`catalog-missing-entry`](#catalog-missing-entry) | catalog | warning | sometimes |
| [`catalog-not-used`](#catalog-not-used) | catalog | warning | sometimes |
| [`catalog-version-divergence`](#catalog-version-divergence) | catalog | warning | none |
| [`unresolved-catalog-references`](#unresolved-catalog-references) | catalog | error | none |
| [`unused-catalog-entries`](#unused-catalog-entries) | catalog | warning | unsafe |
| [`circular-imports`](#circular-imports) | js | off | none |
| [`duplicate-exports`](#duplicate-exports) | js | warning | none |
| [`stale-js-config`](#stale-js-config) | js | warning | none |
| [`stale-mocks`](#stale-mocks) | js | error | none |
| [`unlisted-binaries`](#unlisted-binaries) | js | warning | none |
| [`unlisted-dependencies`](#unlisted-dependencies) | js | error | none |
| [`unparseable-files`](#unparseable-files) | js | warning | none |
| [`unresolved-imports`](#unresolved-imports) | js | error | none |
| [`unused-dependencies`](#unused-dependencies) | js | error | unsafe |
| [`unused-dev-dependencies`](#unused-dev-dependencies) | js | warning | unsafe |
| [`unused-enum-members`](#unused-enum-members) | js | warning | unsafe |
| [`unused-exports`](#unused-exports) | js | warning | unsafe |
| [`unused-files`](#unused-files) | js | warning | unsafe |
| [`unused-namespace-exports`](#unused-namespace-exports) | js | warning | unsafe |
| [`unused-namespace-members`](#unused-namespace-members) | js | warning | unsafe |
| [`unused-namespace-types`](#unused-namespace-types) | js | warning | unsafe |
| [`unused-optional-peer-dependencies`](#unused-optional-peer-dependencies) | js | warning | none |
| [`unused-types`](#unused-types) | js | warning | unsafe |
| [`empty-dependencies`](#empty-dependencies) | manifest | error | safe |
| [`incompatible-peer-dependencies`](#incompatible-peer-dependencies) | manifest | warning | none |
| [`local-path-dependency`](#local-path-dependency) | manifest | error | sometimes |
| [`member-toolchain-fields`](#member-toolchain-fields) | manifest | warning | unsafe |
| [`package-json-format`](#package-json-format) | manifest | off | sometimes |
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
| [`foreign-package-manager-command`](#foreign-package-manager-command) | workspace | warning | none |
| [`missing-package-manifest`](#missing-package-manifest) | workspace | warning | unsafe |
| [`nested-workspace-root`](#nested-workspace-root) | workspace | warning | none |
| [`stale-patched-dependencies`](#stale-patched-dependencies) | workspace | warning | none |
| [`stray-lockfile`](#stray-lockfile) | workspace | warning | unsafe |
| [`toolchain-pin-drift`](#toolchain-pin-drift) | workspace | warning | none |
| [`unmatched-workspace-patterns`](#unmatched-workspace-patterns) | workspace | warning | unsafe |

## catalog-mismatch

A dependency is declared with a version while other packages take it from a catalog.

- Category: catalog
- Default severity: warning
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

## catalog-version-divergence

Packages take one dependency from catalogs that resolve to different versions.

- Category: catalog
- Default severity: warning
- Fixable: none

## unresolved-catalog-references

A `catalog:` specifier names a catalog or an entry that does not exist.

- Category: catalog
- Default severity: error
- Fixable: none

## unused-catalog-entries

A catalog entry that no manifest references; reported only when `[catalog] report-unused-entries` is `true`.

- Category: catalog
- Default severity: warning
- Fixable: unsafe

## circular-imports

Modules import each other in a cycle.

- Category: js
- Default severity: off
- Fixable: none

## duplicate-exports

One binding is exported under several names.

- Category: js
- Default severity: warning
- Fixable: none

## stale-js-config

A `[js]` setting matched nothing or is redundant, a published entry file is missing, or many files are unused.

- Category: js
- Default severity: warning
- Fixable: none

## stale-mocks

A test-runner mock names a relative or aliased module that does not exist.

- Category: js
- Default severity: error
- Fixable: none

## unlisted-binaries

A script, or a command in a workflow, tool config or source file, runs a binary that no listed dependency provides.

- Category: js
- Default severity: warning
- Fixable: none

## unlisted-dependencies

A file imports a package that neither its `package.json` nor any `package.json` above it lists. An import that only the tsconfig `paths` of another package map has the detail `ForeignTsconfigPaths`: it needs a mapping, not a dependency. A package a tool config names (a tsconfig `extends`, an ESLint or Oxlint plugin, a Prisma generator) is reported at the config with the detail `ConfigReference`.

- Category: js
- Default severity: error
- Fixable: none

## unparseable-files

A file reachable from an entry file has a syntax error the parser could not recover from.

- Category: js
- Default severity: warning
- Fixable: none

## unresolved-imports

An import specifier does not resolve to a file or a package.

- Category: js
- Default severity: error
- Fixable: none

## unused-dependencies

A package in `dependencies` is never imported, required or run by a script; `optionalDependencies` are not checked.

- Category: js
- Default severity: error
- Fixable: unsafe

## unused-dev-dependencies

A package in `devDependencies` is never imported, required or run by a script.

- Category: js
- Default severity: warning
- Fixable: unsafe

## unused-enum-members

A member of an exported enum is never read.

- Category: js
- Default severity: warning
- Fixable: unsafe

## unused-exports

An exported value is not imported by any other module.

- Category: js
- Default severity: warning
- Fixable: unsafe

## unused-files

A project file is not reachable from any entry file.

- Category: js
- Default severity: warning
- Fixable: unsafe

## unused-namespace-exports

An exported value of a module that is only read through a namespace import is never accessed.

- Category: js
- Default severity: warning
- Fixable: unsafe

## unused-namespace-members

An exported member of a TypeScript namespace is never read.

- Category: js
- Default severity: warning
- Fixable: unsafe

## unused-namespace-types

An exported type of a module that is only read through a namespace import is never accessed.

- Category: js
- Default severity: warning
- Fixable: unsafe

## unused-optional-peer-dependencies

An optional peer dependency is imported but not installed as a dependency or dev dependency.

- Category: js
- Default severity: warning
- Fixable: none

## unused-types

An exported type or interface is not imported by any other module.

- Category: js
- Default severity: warning
- Fixable: unsafe

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

Private member packages should not declare `packageManager` or `engines`; the root package owns the toolchain.

- Category: manifest
- Default severity: warning
- Fixable: unsafe

## package-json-format

A `package.json` is not formatted as `[format]` describes: key order, sorted fields, `exports` conditions, shorthand `bugs` and `repository`, and the indent. The fix is unsafe when `sort-exports` reorders conditions other than `types` and `default`.

- Category: manifest
- Default severity: off
- Fixable: sometimes

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

A dependency is declared, or resolved by the lockfile, although a banned version group forbids it.

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

Members of a dependency family are on different versions in one package, or as resolved by the lockfile.

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

## foreign-package-manager-command

Dockerfiles, CI workflows and shell scripts must not run a package manager other than the workspace's.

- Category: workspace
- Default severity: warning
- Fixable: none

## missing-package-manifest

A directory matched by the workspace patterns has no `package.json`.

- Category: workspace
- Default severity: warning
- Fixable: unsafe
- sherif name: `packages-without-package-json`

## nested-workspace-root

A workspace member holds a `pnpm-workspace.yaml` of its own, so installs in that directory leave the workspace.

- Category: workspace
- Default severity: warning
- Fixable: none

## stale-patched-dependencies

A `patchedDependencies` entry no longer applies, names a missing file or lies outside the allowed paths, or a patch file is left over.

- Category: workspace
- Default severity: warning
- Fixable: none

## stray-lockfile

A lockfile other than the root lockfile of the package manager: one in a workspace member, or another manager's at the root.

- Category: workspace
- Default severity: warning
- Fixable: unsafe

## toolchain-pin-drift

Node and package-manager versions pinned in version files, Dockerfiles and CI workflows must match the root's.

- Category: workspace
- Default severity: warning
- Fixable: none

## unmatched-workspace-patterns

A workspace pattern does not match any directory.

- Category: workspace
- Default severity: warning
- Fixable: unsafe
- sherif name: `non-existant-packages`

## Unimplemented states

These policy states of ADR 0009 are not emitted yet.

- `DiffersToHighestOrLowestSemverMinor`: a `same-minor` group cannot name a direction, see ADR 0009
- `DiffersToNpmRegistry`: reported by `skif update`, never by a lint rule, see ADR 0058
- `RefuseToCatalogLocal`: a workspace package is never moved into a catalog
