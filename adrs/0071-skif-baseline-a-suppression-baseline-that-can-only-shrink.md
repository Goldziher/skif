---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0001", "0016"]
---

# `skif baseline`: A Suppression Baseline That Can Only Shrink

## Context and Problem Statement

A team that adopts skif on an existing monorepo starts with findings it cannot fix at once. Today it writes
`[[ignore]]` entries by hand ([ADR 0024](0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md)),
or `pinned` version groups, and keeps scripts around them: one that shrinks sherif's `ignoreDependency` list to the
variants still reported, and per-member `pinVersion` groups in syncpack. Nothing tells the team when an entry stopped
matching, and nothing stops the list from growing. Issue [#114](https://github.com/Goldziher/skif/issues/114) asks for
a `skif baseline` command that writes the entries, prunes the ones that match nothing and fails CI when the baseline
gains entries.

How are the current findings recorded, how narrow is each entry, how does a pruned entry get detected, and what does
the ratchet compare against?

## Decision Drivers

* A baseline must silence exactly what was found: a version that was not there when it was written still fails
* The baseline is ordinary configuration, so the lint run, `skif config` and editors read it without a new mechanism
* Writing it twice changes nothing, and its text depends only on the findings, never on the order rules ran in
* The library does no I/O ([ADR 0003](0003-library-first-embedding.md)); reading files, the git history and writing
  belong to the CLI
* The exit codes of [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md): `1` means the check
  found a problem, `2` that the command could not run

## Considered Options

* `[[ignore]]` entries in a separate file that the root `skif.toml` extends, managed by `skif baseline write`,
  `prune` and `check`
* A baseline of finding fingerprints in a file of its own format, read by the lint run (`--baseline <FILE>`)
* Documentation of hand-written `[[ignore]]` entries, with no command

## Decision Outcome

Chosen option: "`[[ignore]]` entries in a separate file that the root `skif.toml` extends", because the entries are
the suppression skif already has, with reasons, `--require-ignore-reasons` and `name@specifier` matching, and a file
reached through `extends` needs no new configuration key or lint flag.

### The baseline file

The baseline is `skif.baseline.toml` in the workspace root, or the file `--file` names. It holds only `[[ignore]]`
entries; any other key is a configuration error, so the commands can rewrite it whole. Each entry is written as

```toml
[[ignore]]
rules = ["version-mismatch"]
packages = ["@acme/web"]
dependencies = ["react@^17.0.2"]
reason = "baseline"
```

* `rules` is the rule of the finding. `packages` is the package's name, or its directory relative to the root when
  another package has the same name. `dependencies` is
  `name@specifier` when the finding carries the specifier as written (every version rule does), so the entry silences
  that version and no other; otherwise the dependency name.
* Characters that are glob syntax in a selector (`*?[]{}`) are written as one-character classes (`[*]`), and a
  leading `!`, which would make the pattern an exclusion, as `{!}` (a class cannot hold it: `[!` opens a negated
  class), so an entry matches the written value literally.
* `reason` is `baseline`, or the text of `--reason`. A reason edited by hand is kept by later writes.
* Errors and warnings are recorded; `info` findings never fail a run and are left out. A finding with neither a
  package nor a dependency is not recorded either: an entry for it would turn its rule off for the whole workspace.
  Nor is a finding in a root package without a name of its own (it has none, or shares it with another package):
  the root's directory is the root itself, so no package pattern selects the root alone, and an entry without one
  would silence the same finding in every package added later. `write` lists such findings on stderr with the cause
  (and in `unrecorded` of the JSON document), and the lint run still reports them; naming the root records them.
* Entries are sorted by rules, packages and dependencies, and identical ones are written once. The file starts with a
  comment saying which commands maintain it. Comments added inside it are not kept.

### `skif baseline write`

`write` resolves the root at `PATH` as linting does, lints it with the configuration files minus the entries of the
baseline file, and adds an entry for every recorded finding that no existing baseline entry matches. Existing entries
stay, including ones that no longer match: removing them is `prune`'s job. The baseline file is written first; then,
when no configuration file of the root already reaches it through `extends`, its path relative to the root's
`skif.toml` (or `.skif.toml`) is added to that file's `extends`: appended to the array with `toml_edit`, or written as
a new `extends = [...]` line at the top (after a `#:schema` line) so the rest of the file stays as it was. A root
without a configuration file gets a `skif.toml` holding that line. Files whose text would not change are not written.
Running `write` again with the same findings changes nothing.

### `skif baseline prune`

`prune` lints the same way and removes every baseline entry that matches no finding of that run. An entry that
matches a finding is kept even when a narrower one would do. The file stays when it is left empty, so the `extends`
that names it keeps resolving. Without a baseline file `prune` exits `2`.

Whether an entry matches a finding is decided by the same code that applies `[[ignore]]` entries during a run:
`skif-lint` exposes `ignore_matches(workspace, entries, diagnostics)`, which returns the findings each entry matches,
and the `skif` facade re-exports it. Rule, package and `name@specifier` matching therefore cannot drift between the
lint run and the baseline commands.

### `skif baseline check`

`check` compares the baseline file with an earlier version of it and exits `1` when it has an entry the earlier
version lacks; entries are compared by their rules, packages and dependencies, not by their reasons. It lints nothing.
The earlier version is

* `--base <REF>`: the file at that git revision, read by running `git -C <dir> show <REF>:./<name>` from the CLI.
  A revision that is not a commit, or that starts with `-`, is a usage error (exit `2`); a file that did not exist at
  the revision is an empty baseline, so a new baseline with entries fails the check.
* `--base-file <FILE>`: that file, for runs without git.

The added entries are printed (human output) or listed in `added` (JSON), with the removed ones in `removed`. A
missing current baseline file is an empty one.

### Output and exit codes

Each command takes `--format human|json`. The JSON document has `version`, `skif`, `command`, `file` (relative to the
root), `entries` (how many the file holds), `added` and `removed` (entries as `rules`, `packages`, `dependencies`
and `reason`); `write` adds `extended` (the configuration file whose `extends` gained the baseline, else `null`) and
`unrecorded` (`rule`, `package`, `dependency`, `message` and `cause`, `workspace-wide` or `unnamed-root`), `write` and
`prune` add `written`, and `check` adds
`base`. `write` and `prune` exit `0` when they finished and `2` when they could not run; `check` exits `0` when the
baseline gained no entry, `1` when it did and `2` when it could not run.

### Spawning git

[ADR 0001](0001-mission-and-scope.md) says the one process the CLI may spawn is the user's package manager, and
[ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) names the install step as the CLI's one
exception to reading files only. `skif baseline check --base` adds a second: it runs `git` to read one file at one
revision. The library still spawns nothing, and no other command runs `git`.

### Consequences

* Good, because the baseline is a plain configuration file: `skif config --show` lists it, its entries have reasons,
  and `--require-ignore-reasons` accepts them.
* Good, because a version entry is exact: a member moving to a third version, or a new member on another version,
  fails the run again.
* Good, because `prune` and the lint run share the matching code, so an entry `prune` keeps is one the run uses.
* Good, because `check --base origin/main` in CI lets the baseline shrink and never grow without a reviewed change.
* Bad, because `[[ignore]]` cannot narrow a finding to a file or an export, so an entry for an `unused-exports`
  finding silences every unused export of that package until it is pruned.
* Bad, because comments written inside the baseline file are lost on the next `write` or `prune`.
* Bad, because `check --base` needs `git` on `PATH` and the revision fetched.

### Confirmation

* `crates/skif-lint/src/ignore.rs` tests that `ignore_matches` returns, per entry, the findings it matches, with
  `name@specifier`, package and rule filters.
* `crates/skif-cli/src/baseline.rs` unit tests cover the rendering, escaping, sorting and the `extends` insertion.
* `crates/skif-cli/tests/baseline.rs` runs the binary: after `write` a plain run exits `0` and a second `write` changes
  nothing; `prune` after a fix removes the entry; `check` fails on growth and passes on shrink, against a file and a
  git revision; a baselined `name@version` still fails when a third version appears.
* The CLI reference test fails when a flag of `skif baseline` is missing from `reference/cli.mdx`.

## Pros and Cons of the Options

### `[[ignore]]` entries in a separate file that the root `skif.toml` extends

* Good, because it reuses the suppression, its matching and its reasons.
* Good, because the baseline is reviewable TOML in its own file, so a diff of it is the diff of the debt.
* Bad, because entries are only as narrow as `[[ignore]]`: rule, package and dependency.

### A baseline of finding fingerprints in a file of its own format, read by the lint run

* Good, because a fingerprint could include the file and the message, and so be narrower.
* Bad, because a fingerprint of a message or a position breaks when the message is reworded or a line moves, and it
  needs a lint flag, a file format and a second suppression path.

### Documentation of hand-written `[[ignore]]` entries, with no command

* Good, because it adds no code.
* Bad, because the scripts the issue describes stay, and nothing detects stale entries or growth.
