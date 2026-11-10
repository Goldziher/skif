---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
---

# Toolchain Pins Outside `package.json` and the `toolchain-pin-drift` Rule

## Context and Problem Statement

The root `package.json` pins the package manager in `packageManager` and the runtime in `engines.node`, and
`member-toolchain-fields` keeps members from declaring their own. The same versions are repeated in files skif never
reads, and they drift silently (GitHub issue #138):

* version files: `.nvmrc`, `.node-version`, `.tool-versions` (`nodejs`), `mise.toml` and `.mise.toml`;
* Dockerfiles: `FROM node:<version>`, `ARG NODE_VERSION=…`, `corepack prepare pnpm@<version>`, `npm i -g pnpm@<version>`;
* CI workflows: the `node-version` of `actions/setup-node`, the `version` of `pnpm/action-setup`.

A workflow that tests on Node 18 while the image ships Node 20, or a Dockerfile that prepares pnpm 8 for a lockfile
pnpm 9 wrote, fails late or not at all. Which files does skif read, what is the source of truth each pin is compared
with, and when do two versions disagree?

## Decision Drivers

* The root package stays the single source of truth for the toolchain, as `member-toolchain-fields` already assumes
* A finding names the file, the line and both versions, so it can be fixed without searching
* A range that admits the root's version is not drift: `node:20` in a Dockerfile follows an `.nvmrc` of `20.11.1`
* skif reads these files for the first time, so a new default must not fail builds that passed before
* Library crates do no I/O; files are read through the `FileSystem` the host provides

## Considered Options

* A `toolchain-pin-drift` rule over a shared scan of toolchain files, on at `warn` by default
* The same rule, off by default
* Extend `member-toolchain-fields` to the other files

## Decision Outcome

Chosen option: "A `toolchain-pin-drift` rule over a shared scan of toolchain files, on at `warn` by default",
because drift is a defect wherever it appears, a warning reports it without failing a build that does not pass
`--fail-on-warnings`, and a separate rule keeps the manifest rule's options and ids unchanged.

**Which files are read.** The rule reads its files through a scan that it shares with every rule over toolchain files
(see [ADR 0088](0088-foreign-package-manager-commands.md)). The usual places are the root and every package
directory that `[workspace] ignore-packages` does not ignore: the version files there, Dockerfiles (`Dockerfile`,
`Dockerfile.*`, `*.Dockerfile`, `*.dockerfile`, `Containerfile`) and `*.sh` there and in their `docker` and
`scripts` subdirectories, and the root's `.github/workflows/*.yml` and `*.yaml`. The `files` option adds globs
relative to the root, read whatever their kind (a YAML file as a workflow, a file of no known kind as a shell script).
A file belongs to the innermost package whose directory holds it, and its findings name that package; a file below a
directory with a `package.json` that is not a workspace package belongs to another project and is not read. The
`kinds` option (`version-files`, `dockerfiles`, `workflows`, `shell-scripts`) picks the kinds read at the usual
places; the default is the first three, since a shell script pins a manager only through a global install.

**What a pin is.** In a version file, the first version of `.nvmrc` or `.node-version` (a leading `v` dropped), the
first version of a `nodejs` (or `node`) line of `.tool-versions`, and the first string of `node` in the `[tools]` table
of `mise.toml`; `pnpm`, `yarn`, `bun` and `npm` (also `npm:pnpm`) entries there pin those managers. In a Dockerfile,
whose instructions are read with their continuation lines, the tag of a `FROM` image whose last path segment is
`node` (up to the first `-`), and `ARG` or `ENV` values of `NODE_VERSION`, `NPM_VERSION`, `PNPM_VERSION`,
`YARN_VERSION` and `BUN_VERSION`. In a workflow step, the `node-version` of `actions/setup-node`, the `version` of
`pnpm/action-setup` and the `bun-version` of `oven-sh/setup-bun`. In the commands of any file (Dockerfile `RUN`,
workflow `run:`, shell scripts), `corepack prepare`, `corepack use` and `corepack install` of `<manager>@<version>`
and a global npm install (`npm install -g pnpm@<version>`). A computed value (`${NODE_VERSION}`,
`${{ matrix.node }}`) and an alias that is no version (`lts/*`, `node:lts`, `latest`) is no pin.

**The source of truth.** For Node, the first root version file that pins it, in the order `.nvmrc`, `.node-version`,
`.tool-versions`, `mise.toml`, `.mise.toml`; otherwise the root's `engines.node`. A root version file that is the
source also has the root's `engines.node` checked against it, reported at that field. For a package manager, the
version of `packageManager` (without its `+sha…` hash), otherwise the `version` of the first known entry of
`devEngines.packageManager`; a pin of another manager is not compared (ADR 0088 covers commands of another
manager). Without a source nothing is compared: the rule does not invent one.

**When two versions disagree.** Both sides are read as node-semver ranges, where an exact version is the range of
itself, and they disagree when no version satisfies both. So an exact pin must equal an exact source and satisfy a
range source, and a range pin (`20`, `20.x`, `^20.1.0`) must admit an exact source or overlap a range source.

**The finding.** It is reported at the pinned version (for a command, the `<manager>@<version>` argument) with the
message "`` `FROM node` pins Node 18.19.0, which does not match 20.11.1 from `.nvmrc` ``", a related location at the
source, the detail `NodeVersion` or `PackageManagerVersion` and advice to change it; for `actions/setup-node` the
advice adds `node-version-file`, for `pnpm/action-setup` removing `version` so the action reads `packageManager`. The
rule has no fix: a pin may be deliberate, and the right repair (reading the version from the file instead) differs per
tool.

**`corepack enable` without a prepare.** A Dockerfile that runs `corepack enable` and no `corepack prepare`, `use` or
`install` gets one finding at the first `corepack enable`, with the detail `CorepackUnpinned` and the severity
`info` unless the rule's level is configured: the manager version then depends on the corepack the image bundles,
unless the project's `package.json` is in place when the manager runs, which skif cannot tell. It is limited to
Dockerfiles because workflows and scripts run in a checkout, where corepack reads `packageManager`.

**Suppressions.** Every kind of file read takes `#` comments. `# skif-ignore-next-line [rule, …] -- <reason>`
suppresses the findings of the named toolchain rules (all of them without names) on the next line that is neither
blank nor a comment, with the lines a trailing backslash continues it onto; `# skif-ignore-file [rule, …] --
<reason>` suppresses them in the whole file. The reason is required: a comment without one suppresses nothing, and
the finding's help says which comment lacks it. The `ignore-files` option takes globs relative to the root, as strings
or as `{ name, reason }` tables like the `[js]` ignore lists of [ADR 0074](0074-reasons-on-js-ignore-entries.md),
and skips those files. `[[ignore]]` entries apply by the package the finding names, `[[overrides]]` by its package
or file.

**Where it lives.** The rule is in `skif-rules` (module `toolchain`), category `workspace`, in a provider of its own,
`toolchain_provider(fs)`, because it reads files through a `FileSystem` and `providers()` takes none; the `skif`
facade registers it with the manifest rules under the `manifest` feature. The shell commands are split by the
tokenizer of the `package.json` script parser, moved to `skif_core::shell` so the rules do not depend on the oxc
crates.

### Consequences

* Good, because a version pinned in an image, a workflow or a version file is checked against the root without a
  custom script.
* Good, because ranges that follow the root (`node:20`, `20.x`) are not reported.
* Good, because hosts that lint in memory pass their own `FileSystem` and get the same findings.
* Bad, because each run lists the package directories and reads the toolchain files, also when the JS analysis is off.
* Bad, because the files are read by line rather than by a full Dockerfile or YAML parser: a flow mapping
  (`with: { node-version: 20 }`), a heredoc `RUN` and a Dockerfile with a backtick escape are not understood.
* Neutral, because a repository whose files drift sees new warnings on upgrade; `kinds`, `ignore-files` and the
  comments narrow them.

### Confirmation

* `crates/skif-rules/src/toolchain/pin_drift.rs`: tests for each pin source, exact and range pins, `engines.node` as
  the source and as a pin, a missing root pin, manager pins from `packageManager` and `devEngines`, the corepack
  finding and its severity, comments with and without reasons, `ignore-files`, `kinds` and `files` globs, other
  projects and ignored packages.
* `crates/skif-rules/src/toolchain/parse.rs`, `scan.rs`, `suppress.rs` and `options.rs`: unit tests of the readers.
* `crates/skif-core/src/shell.rs`: spans across continuation lines.
* `docs/rules.md`, `rule-ids.txt` and `crates/skif-rules/tests/rule-ids.txt` list `toolchain-pin-drift`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A `toolchain-pin-drift` rule over a shared scan of toolchain files, on at `warn` by default

* Good, because users get the check without configuring it, and it does not fail their builds.
* Bad, because a warning is easy to ignore in a run that does not fail on warnings.

### The same rule, off by default

* Good, because nothing changes for anyone on upgrade.
* Bad, because the drift the rule exists to find stays silent until a user reads the changelog.

### Extend `member-toolchain-fields` to the other files

* Good, because the toolchain stays one rule.
* Bad, because that rule judges member manifests and has a `mode` option that means nothing for a Dockerfile, and
  its findings have a fix the new ones cannot have.

## More Information

* Related decisions: [ADR 0007](0007-rule-and-diagnostic-model.md) (rule ids and severities),
  [ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md) (default severities of rules whose
  precision varies), [ADR 0074](0074-reasons-on-js-ignore-entries.md) (ignore entries with reasons),
  [ADR 0088](0088-foreign-package-manager-commands.md) (commands of another package manager, over the same scan)
* See the [ADR index](index.md).
