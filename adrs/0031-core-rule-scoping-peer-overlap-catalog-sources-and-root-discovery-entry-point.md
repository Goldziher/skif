---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Core Rule Scoping: Workspace Protocol Scope, Peer Overlap, Catalog Sources and One Root Discovery Entry Point

## Context and Problem Statement

A review of the core crates found rules and discovery code whose behaviour was either undocumented or at odds with the records that describe it: `workspace-protocol` demanded a protocol that Yarn Classic cannot install, reported peer ranges and repeated `local-path-dependency`; `incompatible-peer-dependencies` judged a dev range by its lowest version; `snap-to` looked its target up in a way no record states; catalogs were read from files of package managers that were not in use; root discovery existed twice, with different answers; and the CLI help for `--ignore-package` did not say that it replaces the configured list. [ADR 0005](0005-workspace-discovery.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) and [ADR 0024](0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md) are locked, so the decisions live here.

## Decision Drivers

* A rule must not demand what the detected package manager cannot install
* A finding should mean a setup that cannot work, not one that a lockfile resolves
* Where syncpack defines a behaviour, skif follows it and says so
* One question, one answer: a recursive run must claim the same directories as the workspace it loads
* Locked records stay unedited

## Considered Options

* State each decision in one new record
* Edit the records that describe the behaviour
* Leave the behaviour undocumented

## Decision Outcome

Chosen option: "State each decision in one new record", because it keeps the records immutable and puts the rule scoping in one place.

**`workspace-protocol` scope (ADR 0009).** The rule is silent when the package manager cannot install `workspace:`: npm and Yarn Classic (1.x, from `packageManager` or the `yarn.lock` header). It stays active when the manager is unknown, because the setting `internal = true` is the user's statement that the protocol is wanted. `peerDependencies` are not reported: a published library names its peers by range. While `local-path-dependency` is enabled, `file:`/`link:`/`portal:` and relative path specifiers are left to it, so one dependency yields one finding; `allow-local-paths` decides them only when that rule is off. The predicate is `Workspace::supports_workspace_protocol`, shared with the suggestion `local-path-dependency` makes.

**`incompatible-peer-dependencies` (ADR 0009).** syncpack and sherif have no such rule, so there is nothing to follow. The rule reports a dev range when no version it allows can satisfy the peer range, judged by range overlap. `^18.0.0` in `devDependencies` against `^18.2.0` in `peerDependencies` is not reported, because a lockfile resolving the dev range installs a version the peer accepts; `~18.1.0` against `^18.2.0` is. An exact dev version is the one version it names. Tags, upper-bounded and compound dev ranges and workspace links are not judged, as before.

**`snap-to` target search (ADR 0009).** The target is looked up among all instances of the dependency in the workspace, not only among those the group selected, as syncpack's `snapped_to` does (it scans every instance and matches the source file of a `snap-to` package). A group restricted to `devDependencies` therefore follows the production declaration of the snap-to package when it has one. Where the package declares the dependency in several sections, production beats development beats any other, and a peer range stated as a compatibility window is never the target.

**`--ignore-package` (ADR 0004, ADR 0016, ADR 0024).** ADR 0024 states that the flag replaces `[workspace] ignore-packages`, and ADR 0016 that array flags replace. That stays; the flag does not append. `--ignore-dependency` adds an `[[ignore]]` entry and so accumulates. The CLI help says that the flag replaces the file's list; `[workspace] extend-ignore-packages` is the appending form in a file.

**Catalog sources (ADR 0005).** Each catalog source is read for the package manager that uses it: `pnpm-workspace.yaml` when the manager is pnpm or unknown, `.yarnrc.yml` when it is Yarn or unknown, and the root `package.json` always. A stale `pnpm-workspace.yaml` in a Yarn repository declares no catalogs, so a fix cannot write into a file the active manager does not read. An unparsable `.yarnrc.yml` is a `WorkspaceError::InvalidYarnrc` in `Workspace::manifest_errors`; the workspace is discovered without its catalogs.

**Yarn Berry detection (ADR 0005).** After lockfiles and `pnpm-workspace.yaml`, a `.yarnrc.yml` or a `.yarn/releases` directory means Yarn, so a checkout that does not commit `yarn.lock` is still recognised.

**Root discovery (ADR 0022, ADR 0023).** `skif_workspace::discover_roots` is removed from the public API. The facade's `discover_targets` is the one pipeline that honours `[workspace] package-manager`, `ignore-packages` and `discover-ignore`; `scan_candidates` and `claim_roots` are the building blocks. A workspace claims every directory its patterns select that holds a `package.json`, including a member whose manifest cannot be parsed (`Workspace::claimed_dirs`), so a broken member is reported once, by its workspace, and is not linted again as a root of its own. A root `package.json` is read as the workspace loader reads it, tolerating a byte order mark, comments and trailing commas. The walk reports the directories it could not list in `Candidates::unreadable`, and one unreadable entry no longer hides its siblings.

**Configuration validation (ADR 0004).** `[workspace] discover-ignore` is validated like the other selectors: a pattern that is not a valid gitignore line is a configuration error naming `workspace.discover-ignore[i]`.

### Consequences

* Good, because the same repository gets the same answer from `workspace-protocol` and `local-path-dependency`.
* Good, because a recursive run and a workspace load agree on what a workspace owns.
* Bad, because `allow-local-paths` has no effect while `local-path-dependency` is on.
* Bad, because overlap is more permissive than the floor version: a dev range that a lockfile could resolve below the peer floor is accepted.

### Confirmation

* `skif-rules` tests cover `workspace-protocol` for each manager, Yarn Classic by field and by lockfile header, peers and paths; the peer table in `incompatible_peer_dependencies.rs` covers overlap.
* `skif-workspace` tests cover a byte order mark and comments in a root manifest, a member with an unparsable manifest, an unreadable directory, catalog sources per manager, an invalid `.yarnrc.yml` and Yarn Berry files.
* `skif-config` tests cover validation of `discover-ignore`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### State each decision in one new record

* Good, because the decisions are findable in one place.
* Bad, because a reader of the older records must follow the index.

### Edit the records that describe the behaviour

* Good, because the facts would sit next to the decisions they correct.
* Bad, because it breaks the immutability that `adrlint.lock` enforces.

### Leave the behaviour undocumented

* Good, because it needs no work.
* Bad, because the behaviour then looks like a defect to the next reviewer.

## More Information

* Amends: [ADR 0005](0005-workspace-discovery.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md)
* Clarifies: [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), [ADR 0024](0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md)
* See the [ADR index](index.md).
