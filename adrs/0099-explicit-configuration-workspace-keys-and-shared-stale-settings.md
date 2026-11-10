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
and discovered files keep their keys relative to the root they configure. The anchor, the root's directory relative
to the start, is a fact of root discovery like the nested-root mark of ADR 0023 (`Workspace::workspace_keys_anchor`),
set for every root below the start of a run with an explicit file. The configuration stays the image of the file, so
the roots built from one file share one linter. The JS analysis reads the anchor when it builds its settings and
matches every `[js.workspaces]` key and `ignore-workspaces` glob through it, and hands each input source the key of
the package it asks about (`InputRequest::workspace_key`), so the plugin tables of `[js.workspaces]` follow without a
setting of their own. Findings spell a package's key relative to the start, so a key in a message is the one to write
in the file. Other selectors (`ignore-packages`, `[[ignore]] packages`, group selectors) and the
override and `extends` paths of ADR 0024 are unchanged.

**Writing the keys.** The keys of an explicit file depend on the directory a multi-root run starts at, not on where
the file lies. The same file linted from the repository root and from `tools/` configures different roots: a table
for the root in `tools/sub` is `[js.workspaces."tools/sub"]` from the repository root and `[js.workspaces."sub"]` from
`tools/`. A file meant for `-r` runs is written for the directory those runs start at, which CI fixes; a
single-workspace run of one root (`skif --config <FILE> tools/sub`) matches the keys from that root, as before.

**`--config` and discovery.** `--config` still skips discovery for every root: a nested root does not read its own
`skif.toml` under the explicit file. Layering discovered files under it would change every run that uses `--config` to
pin its configuration, and the duplicates come from the explicit file alone.

**Findings about a shared file.** `stale-js-config` records, beside its findings, every entry of a configuration list
it judged in a root, used or stale: the file, the list's key as written in it (`js.ignore-dependencies`,
`js.workspaces."tools/*".entry`, `js.workspaces` for the keys of that table), the entry, for a list a `[js.workspaces]`
table gives a package the directory of that package, the verdict and the message of the finding that calls the entry
stale (`Report::config_judgements`, not rendered). A root that cannot judge a list records nothing for it: a rule that
consults it is off, or a package whose entries it covers has entry files that are not all known. The entries two or
more roots judged are decided across them, grouped by absolute file, key as written, entry and, for a package's list,
the absolute package directory: a glob table such as `[js.workspaces."tools/*"]` gives each package it selects a list
of its own, as it does inside one root, so a package that uses its entry does not hide the same entry that a sibling
package's list does not need.

* An entry is stale only when every root that judged it found it stale. A root that used it drops the finding in
  every root; a root that could not judge it has no say, so it does not hide a stale entry either.
* A stale entry is reported once, by the first root in path order whose report holds the finding, at the highest
  severity among the roots' copies, so a copy at `error` is not hidden behind one at `warn`.
* Any other finding at the same file (a redundant pattern, a tag hint) is reported once, by the first root that
  reports it with the same message, at the highest severity among the copies.

**When the decision is made.** The decision is a pure function over the reports a run shows
(`skif::settle_shared_config`, which the CLI calls and an embedder of `discover_targets` and `Linter` calls the same
way), made once and last: after the lint that follows `--fix`, after `--workspace`, `--include` and `--exclude` narrowed
the reports, and after suppressions. No state is carried between lints: each root's report holds what that root found,
and a decision reads only the reports it is given. So a finding the decision would give to a root whose copy a filter
removes goes to the next root that still reports it, and deciding a decided report again changes nothing.

* `--fix` writes every plan before it lints again, then lints again every root a plan changed and every root whose
  configuration is read from a file a plan wrote, so no report that judged a file before it changed takes part.
* `skif baseline` records each root's findings before any decision, so each root's baseline silences the finding it
  reports, and `baseline write -r` run again adds nothing.

The per-root summaries of the JSON `roots` array add up to the run's summary.

### Consequences

* Good, because `[js.workspaces."."]` in an explicit file configures one root, and the recursive run on the corpus
  repository reports nothing for a correct file.
* Good, because one file can carry the settings of every root of a recursive run, keyed by directory as in a knip
  configuration for a monorepo.
* Good, because a stale finding about a shared file means that no root used the setting, for an inherited file as
  well as an explicit one.
* Good, because the roots built from one explicit file share one linter, anchored or not.
* Good, because the decision reads what the rule judged rather than the wording of its messages.
* Good, because the decision is one function over final reports, so the CLI, the language server and an embedder get
  the same findings, and a fix, a filter or a baseline cannot make a finding appear, disappear or repeat.
* Bad, because the keys of an explicit file depend on the directory a recursive run starts at, so a file written for
  runs from the repository root configures other roots, or none, when a run starts elsewhere.
* Bad, because every report carries the entries `stale-js-config` judged, which a single-root run does not need.
* Bad, because a host that lints the roots itself must call the decision, or it reports a shared file's stale entry
  once per root.
* Bad, because each root's baseline records its own copy of a finding about a shared file, so the entries repeat
  across the baselines of the roots that read it.
* Bad, because a finding about an inherited file is located at the nearest configuration file of the root, so a root
  with a file of its own over the inherited one is not grouped with the roots that read the inherited file alone.
* Bad, because the same file now means different things under `--config` and when discovered at the start
  directory of a multi-root run: discovered, its keys stay relative to each root that inherits it.

### Confirmation

* `skif-js` unit tests cover the anchored and unanchored key matching (exact keys, globs, `"."`, unmatched keys) and
  the joining of package directories to the anchor.
* `skif-js` tests cover the judgements `stale-js-config` records (used and stale entries, the key as written, nothing
  for a list it cannot judge) and the key a finding names for a package an override configures below the start.
* `skif` unit tests cover which roots of a discovery get an anchor: every root below the start of a run with an
  explicit file, also when the start is no root, not the root at the start and not a discovered file; and that the
  roots share one configuration.
* `skif` unit tests cover the decision: an entry one root uses is stale nowhere, a root that could not judge an entry
  does not hide it, a stale entry is kept by the first root whose report holds it and at the highest severity of the
  copies, the lists of sibling packages of a glob table are decided apart while one package judged by two roots is one
  list, other findings are kept once, findings of other rules are left alone, and deciding again changes nothing. A
  `skif` test lints the targets of `discover_targets` with `Linter` and gets the finding once after the decision.
* `skif-cli` unit tests cover which roots are linted again after fixing: those a plan changed and those whose
  configuration file a plan wrote.
* CLI tests cover `skif -r --config` on a project with nested roots: `"."` and `"tools/sub"` tables each configure
  their root with no finding; stale entries of the file are reported exactly once; an ignore entry or a glob table
  entry one root uses is not reported; a root that cannot judge a pattern does not hide it; a start directory that is
  no root; a discovered file several roots inherit; the lint after `--fix` reporting as a fresh lint does; the stale
  entry of one package of a glob table reported while a sibling package uses its own; `--workspace` keeping the
  finding of the root it selects; and `baseline write -r` recording the finding in each root, the lint clean after it,
  and a second write adding nothing.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Match the workspace keys of an explicit file from the start directory, and judge findings about a shared file across the roots that are built from it

* Good, because `"."` means one root and every other root is addressed by its directory, as the file's author sees
  the tree.
* Good, because `--config` reads exactly what it read before.
* Bad, because the anchor is one more input of the JS analysis, and the cross-root decision needs the rule to record
  what it judged.

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
