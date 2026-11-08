---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
---

# Safe Version Alignment Fixes: What `--fix` Applies Once the Target Version Is Explicit

## Context and Problem Statement

[ADR 0008](0008-fix-and-edit-model.md) defines a `Safe` fix as one that cannot change behaviour and lists "changing a version" and "replacing a specifier with `catalog:`" as `Unsafe`; [ADR 0009](0009-version-and-catalog-policies.md) repeats that every policy fix is `Unsafe`. `--fix` applies only `Safe` fixes ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)), so `skif --fix` leaves every `version-mismatch` in place: `left-pad` at `1.0.0` in one package and `1.3.0` in another stays misaligned ([#31](https://github.com/Goldziher/skif/issues/31)). `sherif --fix` and `syncpack fix` both align it, and users migrating from either ([ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md)) read `--fix` as "do what sherif did". The issue reports three sherif fixes in the same position: `types-in-dependencies`, `packages-without-package-json` (`missing-package-manifest`) and `non-existant-packages` (`unmatched-workspace-patterns`).

ADR 0008's line is too coarse for version policies. Every version alignment changes what gets installed, but they differ in how much: rewriting `1.0.0` to `1.3.0` is an upgrade semver promises is compatible, to a version the workspace already uses, and `^1.0.0` to `^1.3.0` raises the floor of a range that already admits `1.3.0`; rewriting `~1.2.0` to `~1.3.0` leaves the minor the range was written to stay in, `6.0.0` to `7.0.0` crosses a major release, and `^1.0.0` to `1.3.0` narrows a range to an exact version. The question is which of these a user who typed `--fix` has already agreed to.

## Decision Drivers

* `--fix` should do what sherif's and syncpack's fixes do for the common, low-risk case, so a migration does not lose it
* A fix applied without `--fix-unsafe` must never cross a major release, downgrade, change the kind of range a manifest uses, or move a `~` range out of its minor
* The target version must be one the configuration names or the workspace already uses, never one skif chooses among alternatives on its own ([ADR 0008](0008-fix-and-edit-model.md)); who names it does not make the move itself safer
* `skif --fix` followed by `skif` must leave no diagnostic that `--fix` could still fix
* The rule IDs, the fix operations and the `FixSafety` levels stay as they are; only the classification changes

## Considered Options

* Classify each alignment by its move: `Safe` when the target is explicit and the move is a semver-compatible upgrade of the same shape
* Make every alignment to a `pinned` group's version `Safe`, whatever the move, and classify the others by their move
* Make every fixable version policy state `Safe`, as syncpack does
* Keep every policy fix `Unsafe` and document `--fix-unsafe` as the sherif equivalent
* Add a third safety level between `Safe` and `Unsafe` for version alignment

## Decision Outcome

Chosen option: "Classify each alignment by its move: `Safe` when the target is explicit and the move is a semver-compatible upgrade of the same shape", because it gives `--fix` the alignments that stay inside what each specifier already promises (`left-pad` `1.0.0` to `1.3.0`, the issue's example, and `^1.0.0` to `^1.3.0`) while every move that could break a build, a cross-major upgrade, a downgrade, a prerelease, a change of range or a `~` range leaving its minor, still needs `--fix-unsafe`, and it needs no new flag or level.

A version alignment fix is **`Safe` when its target is unambiguous and explicit, and the move to it is one semver calls compatible**. It stays **`Unsafe` otherwise**. ADR 0008's `Safe` ("cannot change behaviour") becomes, for version alignment, "stays within what semver promises the current declaration is compatible with".

### A compatible upgrade

Rewriting a specifier `from` to `to` is a compatible upgrade when all of these hold:

* both are plain versions or `^`/`~` ranges (`1.2.3`, `1`, `1.2`, `^1.2.3`, `~1.2.3`, `^1`, `~1.2`), and of the same shape: an exact version stays exact, `^1.2.3` stays `^` with three parts. A change of range (`^1.0.0` to `1.3.0`) is the business of semver groups and `semver-range-mismatch`, which stay `Unsafe`;
* neither carries a prerelease or build suffix;
* `to` is the same release as `from` or a later one, never lower, inside semver's compatibility window, the window `^` itself uses: the same major for `1.0.0` and above, the same minor under `0.x`, the same patch under `0.0.x`. This holds for exact versions too (`1.0.0` to `1.3.0`, not `2.0.0`; `0.4.1` to `0.4.9`, not `0.5.0`): semver promises a minor release is compatible, and the target is a version the workspace or the configuration already names;
* a `~` range that names a minor also keeps it (`~1.2.0` to `~1.2.5`, not `~1.3.0`): the range was written to exclude the next minor.

An identical plain specifier (the same text) is always compatible.

An alias (`npm:`), a complex range, `>=`/`<` comparators, `workspace:`, a path, a git URL, `latest` and dist-tags are never compatible upgrades. They keep `Unsafe` fixes, or none ([ADR 0009](0009-version-and-catalog-policies.md)).

### Per state

| Rule | State or fix | Safety |
| --- | --- | --- |
| `version-mismatch` | `DiffersToPin` | `Safe` when the move to the pinned version is a compatible upgrade; otherwise `Unsafe` |
| `version-mismatch` | `DiffersToHighestOrLowestSemver` | `Safe` when the move to the highest (or lowest) version in use is a compatible upgrade; otherwise `Unsafe` |
| `version-mismatch` | `DiffersToSnapTarget`, `DiffersToLocal` | `Safe` when the move to the snap target's or the workspace package's version is a compatible upgrade; otherwise `Unsafe` |
| `catalog-not-used`, `catalog-mismatch` | rewrite to `catalog:` / `catalog:<name>` | `Safe` when the entry exists and holds a plain version or `^`/`~` range that is the instance's own specifier or a compatible upgrade of it; otherwise `Unsafe`, also for an entry of the same text that is a `workspace:`, `npm:`, git or `latest` specifier |
| `catalog-missing-entry` | add the entry, then rewrite | `Unsafe`: skif chooses the entry's version and writes a new entry |
| `banned-dependency` | remove the dependency | `Unsafe` |
| `semver-range-mismatch` | change the range | `Unsafe` |
| `policy-conflict` | the fix of the winning policy | `Unsafe`: two policies disagree, and the configuration is the problem |
| `types-in-dependencies` | move `@types/*` to `devDependencies` | `Unsafe` |
| `missing-package-manifest` | create a `package.json` | `Unsafe` |
| `unmatched-workspace-patterns` | remove the pattern | `Unsafe` |

A pin is no exception. The configuration names the version, but a pin can name another major, a lower version, another range or a prerelease as easily as the workspace can hold one, and the move to it breaks a build just the same; the pin decides where `--fix-unsafe` goes, not whether `--fix` may go there.

`prefer = "lowest"` (or `--select lowest`) asks for a downgrade, which is never a compatible upgrade, so those alignments stay `Unsafe` even though the direction is configured: code may use what the higher version added. `--select` keeps deciding which alternative a selection takes; it does not change a fix's safety.

### The other sherif fixes

* **`types-in-dependencies` stays `Unsafe`.** The rule reports only private packages and `@types/*` packages hold no runtime code, but a private package is still deployed: an image built with `npm ci --omit=dev` (or `pnpm install --prod`) that then type-checks or builds loses the types the move takes out of `dependencies`. skif cannot see the deployment, so the move needs `--fix-unsafe`.
* **`missing-package-manifest` stays `Unsafe`.** Creating a `package.json` adds a member to the workspace: the package manager links it, installs its dependencies and records it in the lockfile.
* **`unmatched-workspace-patterns` stays `Unsafe`.** The pattern matches nothing today, but it records an intent skif cannot see: a directory a generator, a build step or another branch creates. Removing it deletes configuration rather than aligning it.

### Consequences

* Good, because `skif --fix` aligns a workspace on its highest version within each major, as `sherif --fix --select highest` and `syncpack fix` do for those cases.
* Good, because a fix that could break a build (a new major, a downgrade, a narrower range) still asks for `--fix-unsafe`, which neither sherif nor syncpack separates.
* Good, because moving onto a catalog entry that already holds the version is now applied by `--fix`, so adopting a catalog is one command.
* Bad, because one dependency can end half-aligned after `--fix`: the instances within the target's window move, the ones outside it stay and are still reported, with an `Unsafe` fix.
* Bad, because the line between `Safe` and `Unsafe` now depends on the specifiers of each finding, so `version-mismatch` and the catalog rules stay `Fixable::Sometimes` and a user reads the safety per diagnostic (`fixes[].safety` in JSON, `fix (safe)` in the human report).
* Bad, because a semver-compatible upgrade can still break a project whose dependency does not follow semver; ADR 0016's install step and the user's tests are the check, as they are for sherif and syncpack.

### Confirmation

* `crates/skif-rules/src/policy/specifiers.rs` holds a table test of compatible upgrades: same shape, semver's window (`1.0.0` to `1.3.0` but not `2.0.0`, `0.4.1` to `0.4.9` but not `0.5.0`), `~1.2.0` to `~1.2.5` but not `~1.3.0`, no prerelease, no downgrade.
* `crates/skif-rules/src/policy/report.rs` tests the safety of `DiffersToPin` (patch, cross-major, downgrade, range change, prerelease), `DiffersToSnapTarget`, `DiffersToLocal` and `prefer = "lowest"`; `crates/skif-rules/src/policy/catalog/analysis.rs` tests that catalog entries of `latest` and an `npm:` alias are `Unsafe` to move onto although their text matches.
* The conformance fixtures under `fixtures/policy` mark each diagnostic whose fix is `Safe` with `"safe": true` in `expected.json`; both harnesses (`crates/skif-rules/tests`, `crates/skif/tests/policy_conformance.rs`) compare it.
* `fix_applies_compatible_version_alignments_and_leaves_the_rest` in `crates/skif-cli/tests/cli.rs` runs `skif --fix` on `left-pad` 1.0.0 and 1.3.0, `is-number` 6.0.0 and 7.0.0, `ms` `^2.1.0` and 2.1.3, `chalk` 5.3.0 pinned to 5.3.1 and `react` 18.3.1 pinned to 19.0.0, asserts that only `left-pad` and `chalk` are aligned and the `@types/node` entry stays in `dependencies`, and that the re-lint after the fix reports no `Safe` fix.

## Pros and Cons of the Options

### Classify each alignment by its move: `Safe` when the target is explicit and the move is a semver-compatible upgrade of the same shape

* Good, because the common case is fixed by `--fix` and every risky move still needs `--fix-unsafe`.
* Good, because it needs no new level, flag or rule ID.
* Bad, because safety varies per finding of one rule, and a dependency can end partly aligned.

### Make every alignment to a `pinned` group's version `Safe`, whatever the move, and classify the others by their move

* Good, because a pin is explicit configuration, and applying it is what the user wrote.
* Bad, because a pin to another major, a lower version, another range or a prerelease breaks a build as much as the same move chosen by the workspace, and `--fix` would apply it without asking.

### Make every fixable version policy state `Safe`, as syncpack does

* Good, because `--fix` would match `syncpack fix` exactly.
* Bad, because `--fix` would cross majors and downgrade without asking, and `--fix-unsafe` would lose its meaning for the most common rule.

### Keep every policy fix `Unsafe` and document `--fix-unsafe` as the sherif equivalent

* Good, because nothing changes.
* Bad, because `--fix-unsafe` also removes banned dependencies and deletes unused files, so a user who only wanted alignment has to accept much more, and the issue stays open.

### Add a third safety level between `Safe` and `Unsafe` for version alignment

* Good, because the distinction would be visible in the level itself.
* Bad, because `FixSafety` is part of the facade, the JSON and SARIF output and `FixSelection`, and a third level is a breaking change to all of them for a distinction the per-finding classification already makes.

## More Information

* Amends [ADR 0008](0008-fix-and-edit-model.md) ("Safety levels") and [ADR 0009](0009-version-and-catalog-policies.md) ("Fixes are `Unsafe`") for the fixes listed above; the index records the effective statements.
* Related: [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), [ADR 0028](0028-cli-flag-contract-completion-and-withdrawn-confirmations.md), [ADR 0040](0040-version-groups-own-their-instances-the-catalog-rules-leave-claimed-instances.md), [ADR 0048](0048-catalog-version-divergence-and-reasons-on-ignores.md)
* Upstream: [sherif](https://github.com/QuiiBz/sherif), [syncpack](https://github.com/JamieMason/syncpack)
* See the [ADR index](index.md).
