---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0024", "0034"]
---

# Workspace Keys of an Explicit Configuration Are Relative to the Start Directory, and Shared Settings Are Judged Across Roots

## Context and Problem Statement

[ADR 0024](0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md) makes a file given with
`--config <FILE>` (and what it extends) the only configuration of every root and skips discovery. It also keeps the
`[js.workspaces]` keys relative to the root being linted. In a multi-root run the two combine badly: the one file is
applied to each root as if it had been written for that root alone.

Take a project at the start directory with a standalone package nested in `tools/sub`, linted with
`skif . --recursive --config ../cfg/skif.toml`, where the file holds `[js.workspaces."."] entry = ["src/extra.js"]`.
The `"."` table configures the root at the start directory, as intended, and also the root in `tools/sub`, where
`src/extra.js` does not exist. `stale-js-config` ([ADR 0034](0034-js-configuration-parity-with-knip.md)) then reports
the entry as matching nothing, located at the shared file, once for that root. A key such as `"tools/sub"`, written to
configure the nested root, configures nothing: no root has a package at `tools/sub` relative to itself, so every root
reports the key as selecting no package. On a large corpus repository with hundreds of nested roots the run printed
478 such warnings for a file that was correct.

The same duplication exists without `--config` for any file several roots inherit (an organisation file above them):
each root judges the file's ignore lists on its own and reports the entries it did not use, so an entry one root
needs is reported stale by the others, and an entry no root needs is reported once per root.

How does an explicit configuration address the roots of a multi-root run, and when is a setting of a file several roots
share stale?

## Decision Drivers

* One explicit file must be able to configure each root of a recursive run by its directory
* `"."` must mean one root, the one at the start directory
* A setting is stale only when nothing in the run used it, and it is reported once
* `--config` keeps skipping discovery, so a CI run that pins its configuration is not changed by files in the tree
* Keys and positions in findings and in `skif config` output stay those of the file

## Considered Options

* Match the workspace keys of an explicit file from the start directory, and judge findings about a shared file across
  the roots that are built from it
* Layer each nested root's discovered configuration under the explicit file
* Rewrite the keys of the explicit file for each root, dropping those outside it
* Keep the keys relative to each root and only drop duplicate findings

## Decision Outcome

Chosen option: "Match the workspace keys of an explicit file from the start directory, and judge findings about a
shared file across the roots that are built from it", because it gives `"."` one meaning, lets one file configure
every root by its directory, and makes a stale finding mean that nothing in the run used the setting, without
changing what `--config` reads.

**Workspace keys of an explicit file.** In a multi-root run with `--config`, the `[js.workspaces]` keys and the
`ignore-workspaces` globs of the explicit configuration (the file and what it extends) are relative to the directory
the run starts at. A key or glob selects a package of a root when it matches the package directory joined to the
root's directory relative to the start: `"."` is the root at the start directory and no other, `"tools/sub"` is the
root in `tools/sub`, `"tools/sub/packages/*"` its members, and a glob such as `"tools/*"` selects every root it names.
As before, an exact key wins over a glob and a glob never selects the root at the start directory. The root at the
start directory, a single-workspace run and a run without `--config` match keys from the root as ADR 0024 decided,
and discovered files keep their keys relative to the root they configure. The anchor is part of the root's
configuration (`Js::workspaces_anchor`, not a configuration key) and is set only for a root below the start whose
configuration has such keys, so the other roots still share one linter. The JS analysis and the plugin source match
every `[js.workspaces]` key through it; findings spell a package's key relative to the start, so a key in a message is
the one to write in the file. Other selectors (`ignore-packages`, `[[ignore]] packages`, group selectors) and the
override and `extends` paths of ADR 0024 are unchanged.

**`--config` and discovery.** `--config` still skips discovery for every root: a nested root does not read its own
`skif.toml` under the explicit file. Layering discovered files under it would change every run that uses `--config` to
pin its configuration, and the duplicates come from the explicit file alone.

**Findings about a shared file.** Once every root of a run is linted, the `stale-js-config` findings located at a
configuration file that two or more of the linted roots are built from (an explicit file, an ancestor file, a file they
extend) are decided across those roots:

* An entry of a top-level `[js]` list (`ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`,
  `ignore-workspaces`, `entry`, `project`, `ignore`, `ignore-files`) and a `[js.workspaces]` key that selects no
  package are judged by every such root. The finding is kept only when each of them reports it: a root that does not
  report it used the setting, or could not judge it.
* An entry of a `[js.workspaces]` table is judged only by the roots its key selects, and the other hints (a redundant
  pattern, many unused files) describe the root that reports them, so such a finding is kept when a root reports it.

A kept finding is reported once, by the first root in path order that reports it; the other roots are told not to
report it, also when they are linted again after `--fix`. The per-root summaries of the JSON `roots` array add up to
the run's summary.

### Consequences

* Good, because `[js.workspaces."."]` in an explicit file configures one root, and the recursive run on the corpus
  repository reports nothing for a correct file.
* Good, because one file can carry the settings of every root of a recursive run, keyed by directory as in a knip
  configuration for a monorepo.
* Good, because a stale finding about a shared file means that no root used the setting, for an inherited file as
  well as an explicit one.
* Bad, because a root below the start whose configuration has workspace keys gets a linter of its own, so a run with
  many such roots builds more linters and shares less of the file cache.
* Bad, because the decision across roots tells the kinds of finding apart by the configuration key the message names,
  which ties it to the wording of `stale-js-config`.
* Bad, because an entry of a glob table that selects packages of several roots is reported when one of them does not
  use it, even if another does: only the entries of top-level lists and the keys are judged across roots.
* Bad, because the same file now means different things under `--config` and when discovered at the start
  directory of a multi-root run: discovered, its keys stay relative to each root that inherits it.

### Confirmation

* `skif-js` unit tests cover the anchored and unanchored key matching (exact keys, globs, `"."`, unmatched keys) and
  the joining of package directories to the anchor.
* `skif` unit tests cover which roots of a discovery get an anchor: the roots below the start with an explicit file
  that has `[js.workspaces]` keys or `ignore-workspaces` globs, not the root at the start, not a discovered file, not
  a file without such keys.
* `skif-cli` unit tests cover the decision across roots: a finding every root reports is kept once, one reported by a
  single root of two is dropped when every root judges it and kept otherwise, and a re-lint drops what another root
  decided.
* CLI tests cover `skif -r --config` on a project with a nested root: `"."` and `"tools/sub"` tables each configure
  their root with no finding; stale entries of the file are reported exactly once; an ignore entry the nested root uses
  is not reported by the outer root.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Match the workspace keys of an explicit file from the start directory, and judge findings about a shared file across the roots that are built from it

* Good, because `"."` means one root and every other root is addressed by its directory, as the file's author sees
  the tree.
* Good, because `--config` reads exactly what it read before.
* Bad, because the anchor is one more input of the JS analysis, and the cross-root decision depends on message keys.

### Layer each nested root's discovered configuration under the explicit file

* Good, because a nested project keeps settings it writes for itself.
* Bad, because it changes the contract of `--config` for every user: a run that pins its configuration starts reading
  files in the tree, which is what `--config` exists to prevent, and oxlint's `--config` disables nested lookup for
  the same reason.
* Bad, because it does not remove the duplicates: the explicit file's `"."` table still applies to every root.

### Rewrite the keys of the explicit file for each root, dropping those outside it

* Good, because roots that end with the same rewritten tables share a linter.
* Bad, because a glob whose leading directories are not literal (`"*/sub"`, `"**/web"`) cannot be rewritten exactly
  for a root below the start.
* Bad, because the keys in findings and in `skif config` output would no longer be the keys of the file.

### Keep the keys relative to each root and only drop duplicate findings

* Good, because it is the smallest change.
* Bad, because `"."` keeps configuring every root, a key that names a nested root still configures nothing, and an
  entry one root needs is still reported by the others.

## More Information

* Amends: [ADR 0024](0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md) (the
  `[js.workspaces]` keys and `ignore-workspaces` globs of an explicit file in a multi-root run are relative to the
  start directory), [ADR 0034](0034-js-configuration-parity-with-knip.md) (a `stale-js-config` finding about a file
  several roots share is decided across them)
* Related decisions: [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md),
  [ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md),
  [ADR 0085](0085-files-other-roots-reach-count-as-used.md)
* Prior art, as documented when this record was written: knip, [Monorepos and
  workspaces](https://knip.dev/features/monorepos-and-workspaces), keys its `workspaces` map by directory relative to
  the root configuration file, with `"."` the root workspace.
* See the [ADR index](index.md).
