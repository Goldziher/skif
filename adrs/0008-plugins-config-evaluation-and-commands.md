---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# Plugins, Config Evaluation and Commands

## Context and Problem Statement

Without knowledge of the tools a project uses, the JS analysis reports their config files as unused, the packages
they load as unused dependencies and the binaries their scripts run as unlisted. knip solves this with plugins that
mostly load and execute each tool's config. skif executes nothing
([ADR 0007](0007-js-analysis-core-on-oxc.md)), so it must read configs statically, and real configs compute values:
they merge a shared base, branch on the environment and call project code. A value read wrongly is worse than one not
known, since a wrong entry list reports used files as unused and a fix may delete them. Commands are another source of
use: scripts, CI steps, hooks, tool configs and spawn calls in source files each run binaries in a directory that is
often not where the command is written.

How is a plugin expressed and grown, what does the config evaluator compute, what happens when it cannot know, and in
which package is a command's binary judged?

## Decision Drivers

* Most plugins are a few lines of data, reviewable without reading code, and free unless enabled
* A value the evaluator reports as known is never one the config's JavaScript could change; imprecision is allowed,
  unsoundness is not
* Unknown entries never turn into unused-file findings, and only what an unknown config could reach stays unjudged
* A binary is judged against the manifest whose dependencies the command uses, at the file that runs it
* A deviation from knip never reports a file or dependency that is in use

## Considered Options

* A declarative plugin table with static resolvers, a sound static evaluator with incompleteness scoped per cause, and
  a script parser that tracks working directories
* One hand-written resolver per plugin, which makes every plugin code to review
* Execute tool configs, as knip does, which runs untrusted code
* Treat any unknown value as "the whole package is unjudged", which hides real dead code

## Decision Outcome

Chosen option: "A declarative plugin table with static resolvers, a sound static evaluator with incompleteness scoped
per cause, and a script parser that tracks working directories", because most plugins stay data, nothing is executed,
no file an unknown config may load is reported, and each binary is settled from facts the repository already holds.

### The plugin model

* A plugin is a row of the table in `skif-js-plugins` behind the `Plugin` trait: `spec()` returns a declarative
  `PluginSpec` (id, enablers, config, entry, production and project patterns, binaries, the knip commit it was checked
  against), and optional hooks resolve a statically evaluated config, a workspace, or a config that is a script.
* A resolver returns `PluginInput`s, a `#[non_exhaustive]` vocabulary of what a config can say: entries, dependencies,
  binaries, scripts, aliases, linked configs, shared configs and the like. Shared extraction helpers in
  `skif-js-syntax` read what knip reads from a loaded config, so plugins do not each walk the AST.
* A plugin is enabled by a dependency, its package name, a file, a script, a `package.json` key, or always.
  `[js.plugins] <id>` forces it on or off or overrides its patterns. There are no user-defined plugin rows.
* Plugins run in passes: before the module graph, a plugin whose findings come from its package's sources searches no
  file; after it, it searches only the reached files, from the texts the graph already read. A command in a file
  nothing reaches runs nothing.
* A config one package ships for the packages that extend it is a shared config: the shipping package's manifest is
  judged for what it names, and what describes the extending projects is left out. Heft rigs are the general case: the
  installed rig's selected profile holds configs of the project, and what they name is credited to the project.
* A package a plugin loads at run time without the project naming it is credited, never required. Test files are
  entries only through a test-runner plugin or `[js] entry`.

### Growing the set and deviating from knip

* A plugin is added when it is used in the corpus or fixes a reported false positive, with a test that fails without
  it. Its id is knip's id for the same tool. A tool without a knip counterpart may have a row when the tool documents
  the behaviour. A row whose knip counterpart contributes only an enabler is not added.
* A plugin follows knip's plugin of the same id unless a deviation only widens credit: where knip executes, skif reads
  the static form and an unreadable part makes entries incomplete rather than guessed; approximations err towards more
  entries and more credited dependencies; a command whose meaning knip's reading would change is skipped. Each
  deviation is stated in the plugin module and pinned by a test.

### The evaluator

* The evaluator in `skif-js-syntax` computes `ConfigValue`s from literals, objects, arrays, same-file constants,
  imports it follows and calls it understands. Anything else is `ConfigValue::Unknown`, or an object with an unknown
  spread or part.
* Soundness rule: a write through an alias, a call of a function that may change a module's value, and a module
  imported for its effects leave the values they may reach unknown. Imports of project code are not trusted; helpers
  such as `define…` and copying merges are trusted only from installed packages.
* `ConfigValue::OneOf` holds a few known alternatives from a branch with an unknown test. It reads as unknown unless a
  plugin asks for its alternatives; plugins that read lists of names credit every alternative.
* A test that reads `process.env` or `import.meta.env` and stays undecided makes the config evaluate twice more, with
  every such test true and then false. A package required on only one side becomes an optional reference: it credits a
  manifest that lists it and is never unlisted.

### Incomplete entries

* A config that evaluates to an unknown value, has an unknown spread, computes its entry keys or does not parse makes
  its package's entries incomplete. So does a published path or plugin entry in build output that maps to no source.
  Each cause is reported per package in `incomplete-packages`
  ([ADR 0012](0012-cli-contract-and-reporting.md)).
* `Plugin::incomplete_scope` says where hidden entries can lie: the whole package, or globs within it. A plugin
  narrows only for what a shared base can set, because a base can name files only where the tool looks by convention;
  a config the project computes itself may name any file and leaves the whole package unjudged.
* In an incomplete package the files of the scope are graph entries; files outside it, and their exports and members,
  are judged as in a complete package; the exports of a module inside the scope stay unjudged, also with
  `include-entry-exports`. Its dependencies are never judged. A non-static `import()` or `require()` drops
  the scope.
* A missing path in a package's build output stands for its source wherever it is named (a published path, a plugin
  entry, a relative import), mapped through the `tsconfig` output directories and the conventional output-to-source
  twins. A published path into nothing that no build may write is no entry and is reported by `stale-js-config`.
* `explain-usage` names each case: the scope that made a file an entry, and the verdict `not-judged`.

### Commands

* The script parser reads POSIX shell with the shared tokenizer `skif_core::shell`. `cd` and `pushd` move the commands
  after them; a subshell, `sh -c` and a command substitution end their own directory changes; `npm --prefix`,
  `pnpm -C` and `yarn --cwd` move the command they run. Here-document bodies, `case` patterns and functions the script
  defines run no binary.
* Working directories outside the shell act as a leading `cd`: a CI step's `working-directory` and the `cwd` option of
  a spawn call, resolved below the file when it is computed from the file's own location. A directory that is not a
  static relative path is unknown.
* A command belongs to the innermost workspace package whose directory it runs in. A binary that package does not
  provide is reported by `unlisted-binaries` at the command, naming where it runs, unless the package has a script of
  that name. A listed dependency named like the binary provides it only while its manifest is unknown; once known, its
  `bin` field decides. A script run in another package is a script of that package. A binary run in a named workspace (`pnpm --filter X`) belongs to `X`.
* In an unknown directory, a binary run by name is judged where the command starts, and a package manager run there is
  treated as filtered: its scripts are not followed and its binaries are not judged.
* Judged commands: `package.json` scripts, every command a plugin reads, and the literal `child_process`, `execa`,
  `zx` and nano-spawn commands of reached files, read during the parse the analysis already does. A file such a
  command runs becomes an entry, and the graph is rebuilt, for a bounded number of rounds, until it reaches no new
  file.
* Commands on a machine that brings its own tools (CI workflows, Git hooks, task runners) mark their binaries
  optional: a binary run by name there is a credit only, while one reached through a package manager is judged.
  Binaries fetched on demand (`npx`, `pnpm dlx`) are credits only.

### Consequences

* Good, because most plugins are data, cost nothing unless enabled, and are checked by one test each.
* Good, because no finding rests on a value a config could change, and no file an unknown entry may load is reported or
  deleted, while one unknown key does not hide every dead file of its package.
* Good, because `cd sub && pnpm exec rspack` credits `@rspack/cli` in `sub` from a script, a workflow or a source file
  alike, and a missing binary is reported at the command.
* Good, because spawn and source-scan commands come from the existing parse, so no file is read twice.
* Bad, because plugins whose knowledge exists only in the evaluated config degrade to their static reading, and every
  plugin follows its tool's releases.
* Bad, because soundness costs precision, and the dependencies of an incomplete package are never judged.
* Bad, because a superset of entries can hide an unused file, and a command in a file nothing reaches credits nothing.
* Bad, because a binary a package manager runs in an unknown directory is not reported where it is missing, and a tool
  a CI step runs by name is never reported.

### Confirmation

* `scripts/check-plugin-list.sh` compares the README and website plugin lists with the table.
* `crates/skif-js-plugins/tests/plugins.rs` pins each plugin and each deviation from knip;
  `crates/skif-js/tests/heft_rigs.rs` and `crates/skif-js/tests/reached_sources.rs` cover rigs and the plugin passes.
* `crates/skif-js-syntax/tests/config_functions.rs`, `crates/skif-js-syntax/tests/config_imports.rs` and
  `crates/skif-js-plugins/tests/fp_config.rs` hold the soundness rule and environment variants.
* `crates/skif-js/tests/build_output_entries.rs`, `crates/skif-js/tests/config_hints.rs` and
  `crates/skif-js/tests/explain_absence.rs` cover incomplete scopes, build output and their explanation.
* `crates/skif-core/src/shell.rs`, `crates/skif-js-plugins/src/scripts/tests.rs` and `crates/skif-js/src/scripts.rs`
  cover the parser and the package a command runs in; `crates/skif-js/tests/unlisted_parity.rs` covers the binaries
  judged.
