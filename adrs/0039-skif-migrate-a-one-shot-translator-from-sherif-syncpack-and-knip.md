---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# `skif migrate`: A One-Shot Translator from sherif, syncpack and knip Configuration

## Context and Problem Statement

[ADR 0001](0001-mission-and-scope.md) and [ADR 0004](0004-configuration-model.md) say that skif has no compatibility reader for sherif, syncpack or knip configuration, and that users migrating from those tools rewrite their configuration by hand. The comparison page and the configuration reference repeat it. A repository with a knip workspace map, syncpack version groups and a sherif ignore list has dozens of constructs to carry over, most of which translate one to one, and a hand rewrite loses the ones that do not without telling anyone.

skif's static extraction of JavaScript and TypeScript configuration ([ADR 0010](0010-js-analysis-on-oxc.md)) already reads the files these tools use, including `knip.ts`, without executing them. What is missing is a translation from their keys to the skif model and a report of what could not be translated.

## Decision Drivers

* Moving to skif must not silently lose configuration
* skif must keep reading `skif.toml` only: one parser, one precedence rule ([ADR 0004](0004-configuration-model.md))
* Nothing in a user's repository may be deleted or run
* The old tools check less than skif does by default, and a migration that changes what is checked must say so
* The translation must be testable on in-memory text, like every other library crate ([ADR 0003](0003-library-first-embedding.md))

## Considered Options

* A new `skif-migrate` crate and a `skif migrate` command that translate once and report what needs a human
* A compatibility reader: skif reads `.syncpackrc`, `knip.json` and the `sherif` key at lint time
* Documentation of the translation only, with the rewrite by hand

## Decision Outcome

Chosen option: "A new `skif-migrate` crate and a `skif migrate` command that translate once and report what needs a human", because it keeps the loader and the model unchanged, carries over what translates, and turns every construct that does not into a named note instead of a silent loss.

**A translator, not a reader.** `skif migrate` writes a `skif.toml`. skif never reads the old files while linting, so ADR 0001's statement that there is no compatibility reader stays true; what changes is its consequence that migration is by hand. `--remove-old` does not exist and the command deletes nothing.

**Sources.** sherif: the `sherif` key of `package.json`. syncpack: `.syncpackrc` and its `json`, `yaml`, `yml`, `js`, `cjs`, `mjs`, `ts` variants, `syncpack.config.*`, the `syncpack` key. knip: `knip.json(c)`, `.knip.json(c)`, `knip.*` and `knip.config.*` in JavaScript and TypeScript, the `knip` key. The flags of the `package.json` scripts that run each tool are translated too, because they change what the tool checked (`knip --dependencies --production`, `syncpack lint --dependency-types prod,dev`, `sherif --fail-on-warnings -i react`).

**Static only.** JavaScript and TypeScript files go through `skif-js-syntax`, as plugin configuration does. A value that is not static (a call, `process.env`, an import) is not guessed: its key is left out and the report names it as needing a human, and the other keys are still translated. `skif-js-plugins` makes `load_config` and `scripts::split_commands` public for this.

**Typed target, checked output.** The output is built as text, so it can carry comments and a fixed order, and is parsed back with `Config::from_toml_str` before it is returned; a failure there is a bug in `skif-migrate`. Keys that other changes are adding to the model are listed in one table (`keys.rs`) and are removed from the copy that is validated until the model has them. The workspace has no format-preserving TOML editor, so `--merge` appends the sections an existing `skif.toml` lacks after its text and only merges parsed values, losing the existing comments, when both files define the same table.

**Profiles.** Because sherif checks ten things and skif has more rules enabled by default, the default `exact` profile turns every other default-on rule off in `[rules]` and disables the JS analysis when knip is not migrated; `full` keeps skif's defaults. Both write every rule that knip enabled at knip's severity (`error` unless its `rules` key says otherwise), because skif's own defaults are mostly warnings. The report lists the extra checks either way.

**Honest classes.** Every note is one of: translated one to one, translated approximately (skif behaves differently, and the note says how), covered by skif by default, or needing a human (not static, unclear, or no skif equivalent). `--strict` turns the last group into exit code 1. A group whose selector cannot be translated is not written, because a group without its selector would match every dependency.

### Consequences

* Good, because a migration is one command and every untranslated construct is named with its key path.
* Good, because the loader, the model and the precedence rules are untouched.
* Good, because `exact` keeps a migrated repository's results comparable with the old tools.
* Bad, because the translation tables follow three moving upstreams and need an update when a tool adds a key.
* Bad, because several constructs only approximate (a knip `ignoreFiles` entry hides a file from every rule, a sherif `name@version` ignore matches the diagnosed instance) and the user has to read the notes.
* Bad, because `--merge` into a file that defines the same tables loses its comments.

### Confirmation

* Golden tests translate a realistic configuration per tool and compare the generated `skif.toml` with a snapshot; the output of each is parsed as a skif configuration.
* A test checks that every rule id the translation tables name exists in `rule-ids.txt` and that the sherif renames agree with `retired-rule-ids.txt`.
* Merging the output of a migration into itself changes nothing.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A new `skif-migrate` crate and a `skif migrate` command that translate once and report what needs a human

* Good, because the cost is paid once per repository, not on every lint run.
* Good, because the library is pure and testable on in-memory text.
* Bad, because it adds a crate and a command.

### A compatibility reader: skif reads `.syncpackrc`, `knip.json` and the `sherif` key at lint time

* Good, because nothing needs to be migrated.
* Bad, because skif would carry three more configuration formats and their semantics for ever, and a repository would have two sources of truth.
* Bad, because semantics that differ between the tools (scope of an ignore, severity defaults) could not be reported to the user.

### Documentation of the translation only, with the rewrite by hand

* Good, because it needs no code.
* Bad, because a hand rewrite loses constructs without telling anyone.

## More Information

* Supersedes the consequence "migrating users rewrite configuration by hand" of [ADR 0004](0004-configuration-model.md) and the matching sentence of [ADR 0001](0001-mission-and-scope.md); both still say that skif has no compatibility reader.
* Adds the crate `skif-migrate` to the list of [ADR 0002](0002-crate-layout-and-publishing.md).
* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
