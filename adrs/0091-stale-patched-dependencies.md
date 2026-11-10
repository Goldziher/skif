---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
---

# `stale-patched-dependencies`: Patches That No Longer Apply, Missing or Misplaced Files and Leftover Patches

## Context and Problem Statement

pnpm applies the patches `patchedDependencies` lists (in the root `pnpm-workspace.yaml`, or `pnpm.patchedDependencies`
of the root `package.json`) and fails an install only when a patch file is missing. The entries go stale unnoticed in
three other ways:

* the key names a `name@version` the lockfile no longer resolves, so after a bump the patch silently stops applying;
* the patch file lies outside the paths a build context copies (a Docker context that whitelists `patches/`), so the
  install fails only in the image;
* a patch file is left behind after its entry is removed.

Which of these does skif report, and from which files?

## Decision Drivers

* A stale patch is a correctness problem, not a style one, but an existing workspace must not start failing on a
  leftover file
* The lockfile is read as [ADR 0090](0090-lockfile-scopes-for-banned-groups-and-dependency-families.md) reads it: on
  demand, pnpm format 9 only, and a check that cannot run says so
* A workspace that declares no patches pays nothing
* Diagnostics point at the key a user edits

## Considered Options

* One rule over the declared entries, the lockfile and the patch directories
* Separate rules for applied patches, missing files and leftover files
* Leave patches to pnpm

## Decision Outcome

Chosen option: "One rule over the declared entries, the lockfile and the patch directories", because the three
findings share their input and their fix (edit `patchedDependencies` or the patch files), and one rule ID is one
switch.

**Rule.** `stale-patched-dependencies`, category `workspace`, default severity `warning`, no fix. It reads the
`patchedDependencies` of the root `pnpm-workspace.yaml`, then `pnpm.patchedDependencies` of the root `package.json`;
an entry of the YAML file is located from its key through its value, one of the JSON file at its key. A workspace that
declares no entries and configures no `patch-dirs` is not looked at further.

**Findings.**

* **Not applied.** For a key `name@version` with an exact version, the lockfile must hold a snapshot of that release; a
  key without a version, or with a range, must find some release of `name`. Without one, the entry is reported with the
  releases the lockfile resolves instead. With one, a snapshot of it must carry a `patch_hash` and the lockfile's
  `patchedDependencies` must record a hash for the key; otherwise the patch is reported as not applied.
* **Missing file.** The file of an entry, relative to the root, does not exist.
* **Outside the allowed paths.** With `allowed-patch-paths` (globs relative to the root, `*` crossing `/` as in every
  skif selector), a file that matches none is reported. Empty (default) allows any path.
* **Leftover file.** A `*.patch` file directly in a patch directory that no entry references (paths compared after
  normalising `./` and `..`) is reported at the file. The patch directories are `patch-dirs` when configured, else the
  directories of the referenced patches and `patches`.

When the lockfile check cannot run (no `pnpm-lock.yaml`, another manager's lockfile, a lockfile that does not parse),
the rule reports that once and still checks the files.

### Consequences

* Good, because a bump that drops a patch, a patch a build context does not copy and a leftover patch are each
  reported where they are fixed.
* Good, because the default `warning` keeps a leftover file from failing an existing CI run.
* Bad, because a `patches/` directory shared with patch-package (`name+1.0.0.patch`) next to pnpm patches reports the
  patch-package files as leftovers; `patch-dirs` narrows the search.
* Bad, because the patch directories are searched one level deep.

### Confirmation

* `skif-rules` unit tests in `manifest/stale_patched_dependencies.rs`: an applied patch, a key whose release the
  lockfile no longer resolves (located at the key), a resolved release without a patch hash, a missing file, a
  leftover file with the default and a configured `patch-dirs`, `allowed-patch-paths`, an entry of `package.json`
  located at its key, a missing and a broken lockfile, a workspace without patches reading nothing, and invalid
  options.
* The `dirty` fixture of `skif-rules` declares a patch whose file is missing, so the rule fires in the snapshot test.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### One rule over the declared entries, the lockfile and the patch directories

* Good, because one switch and one place in the documentation cover the whole entry.
* Bad, because a project that wants only the leftover check cannot turn the others off.

### Separate rules for applied patches, missing files and leftover files

* Good, because each could have its own severity.
* Bad, because three rule IDs to describe one table, and every new public ID is permanent
  ([ADR 0007](0007-rule-and-diagnostic-model.md)).

### Leave patches to pnpm

* Good, because nothing to maintain.
* Bad, because pnpm reports only the missing file, and only on install.

## More Information

* Related decisions: [ADR 0007](0007-rule-and-diagnostic-model.md),
  [ADR 0090](0090-lockfile-scopes-for-banned-groups-and-dependency-families.md)
* Issue: [#142](https://github.com/Goldziher/skif/issues/142)
* See the [ADR index](index.md).
