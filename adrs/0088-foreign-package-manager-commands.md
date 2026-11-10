---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
---

# Commands of Another Package Manager and the `foreign-package-manager-command` Rule

## Context and Problem Statement

After a monorepo moves to pnpm (or yarn), `npm ci`, `npm install`, `npm run`, `npx` and `yarn` calls survive in
Dockerfiles, CI workflows and shell scripts (GitHub issue #139). They fail late, at image build or deploy time, often
on `catalog:` or `workspace:` specifiers npm cannot read, or they install from a lockfile nobody updates. skif knows
the workspace's package manager but reads none of these files.

Which commands count as running another package manager, which do not, and how is a deliberate one kept?

## Decision Drivers

* The workspace's package manager is the one detection already decides (`[workspace] package-manager`,
  `packageManager`, `devEngines.packageManager`, lockfiles), so every rule agrees on it
* A command that only reads (`npm view`, `npm whoami`) or that installs the chosen manager itself is no defect
* A project below the repository that is not a workspace package may keep its own manager
* A deliberate exception needs a reason next to it, per file or per line
* The files are read by the scan `toolchain-pin-drift` uses ([ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md)), not a second reader

## Considered Options

* Report the install, run and exec commands of another manager, with read-only commands and global installs of the chosen manager exempt, on at `warn`
* Report every invocation of another manager
* Report only installs (`npm ci`, `npm install`, `yarn install`)

## Decision Outcome

Chosen option: "Report the install, run and exec commands of another manager, with read-only commands and global
installs of the chosen manager exempt, on at `warn`", because those are the commands that break once the lockfile and
specifiers belong to another manager, while the exempt ones work whatever the workspace uses.

**Which commands are read.** The simple commands of Dockerfile `RUN` instructions (shell form with its continuation
lines, and exec form `["npm", "ci"]`), of workflow `run:` steps (scalars and block scalars) and of shell scripts,
including those inside `$( )` and backticks, split by the tokenizer of the `package.json` script parser
(`skif_core::shell`). `CMD` and `ENTRYPOINT` are not read: they run the built image, not the build. The program is
found past reserved words (`then`, `do`), `sudo`, `env NAME=value`, `exec`, `time`, `nohup` and `command`; a computed
program (`$PM install`) is not judged. The files are those of ADR 0087 with the default `kinds` Dockerfiles,
workflows and shell scripts; the same `kinds`, `files` and `ignore-files` options apply.

**What is reported.** With a workspace manager other than npm: `npm` with `install`, `i`, `ci`, `add`, `run`,
`run-script`, `exec`, `test`, `start` and their aliases (also `uninstall`, `update`, `rebuild`, `dedupe`, `prune`,
`link`), and `npx`. With one other than yarn: a bare `yarn` (which installs), and `yarn` with any subcommand or script
name that is not read-only. With one other than pnpm: `pnpm` and `pnpx` likewise. With one other than bun: `bun`
`install`, `add`, `remove`, `update`, `link` and `bunx`; `bun run` and `bun <file>` are not reported, since bun is
also a runtime. `corepack <manager> …` counts as that manager. The finding is at the program through its subcommand
(`npm ci`, `npx tsc`), with the message "`` `npm ci` runs npm, but the workspace's package manager is pnpm ``", the
detail naming the manager run (`npm`), and advice naming the chosen manager's equivalent (`pnpm install
--frozen-lockfile`, `pnpm dlx`) or the comment that keeps it. The rule has no fix: the flags of the commands differ
between managers.

**What is not reported.** A command that only prints `--version` or `--help`. npm subcommands outside the list above
(`view`, `whoami`, `publish`, `pack`, `config`). Read-only subcommands of yarn and pnpm (`info`, `view`, `why`,
`list`, `config`, `cache`, `audit`, `outdated`, `npm`, `publish`, `pack`, `whoami` and the like). A global install
that installs only the chosen manager or corepack (`npm install -g pnpm@9 corepack`, `yarn global add pnpm`). A
runner that runs the chosen manager (`npx pnpm@9 install`, `yarn dlx pnpm`). A file below a directory with a
`package.json` that is not a workspace package, or below an ignored package, as ADR 0087 decides for every toolchain
rule. Without a detected package manager nothing is reported.

**Deliberate exceptions.** `# skif-ignore-next-line foreign-package-manager-command -- <reason>` above the line,
`# skif-ignore-file foreign-package-manager-command -- <reason>` anywhere in the file, or an `ignore-files` entry
with a `reason`, as ADR 0087 defines them; a comment without a reason suppresses nothing. `[[ignore]]` entries apply
by the package the finding names.

**Default.** `warn`, like `toolchain-pin-drift` and for the same reason: the files are new to skif, and a warning
reports the commands without failing a build that does not pass `--fail-on-warnings`.

### Consequences

* Good, because a Dockerfile or workflow still calling npm after a move to pnpm is reported where the call is.
* Good, because version checks, registry reads and installing the chosen manager stay quiet.
* Good, because both toolchain rules read each file once.
* Bad, because `yarn <script>` and `pnpm <script>` are reported by script name, so a program called `yarn` or `pnpm`
  that is not the package manager would be reported too.
* Bad, because a command run in a directory that keeps another manager on purpose, but is not a separate project
  with its own `package.json`, needs a comment or an `ignore-files` entry.
* Neutral, because `CMD`, `ENTRYPOINT` and heredoc `RUN` bodies are not read.

### Confirmation

* `crates/skif-rules/src/toolchain/commands.rs`: `invocations_of_another_manager_are_found` covers each command form
  and each exemption; `the_chosen_manager_decides_what_is_foreign` covers other workspace managers.
* `crates/skif-rules/src/toolchain/foreign_command.rs`: tests for every file kind, the exemptions and other projects,
  comments with and without reasons, `ignore-files`, `kinds`, `[[ignore]]` and the detected manager.
* `crates/skif-rules/tests/toolchain.rs`: both toolchain rules over an on-disk fixture, with a snapshot.
* `docs/rules.md`, `rule-ids.txt` and `crates/skif-rules/tests/rule-ids.txt` list `foreign-package-manager-command`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Report the install, run and exec commands of another manager, with read-only commands and global installs of the chosen manager exempt, on at `warn`

* Good, because it reports what breaks and leaves what works.
* Bad, because the lists of subcommands must follow the managers' command sets.

### Report every invocation of another manager

* Good, because it is simple and needs no lists.
* Bad, because `npm view pnpm version` and `npm install -g pnpm` in a pnpm image would be findings that are correct
  code.

### Report only installs (`npm ci`, `npm install`, `yarn install`)

* Good, because installs are the commands that fail most visibly.
* Bad, because `npm run` and `npx` keep running scripts and tools through the wrong manager, and a bare `yarn` is an
  install by another name.

## More Information

* Related decisions: [ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md) (the
  shared scan, the options and the suppression comments), [ADR 0074](0074-reasons-on-js-ignore-entries.md) (ignore
  entries with reasons), [ADR 0007](0007-rule-and-diagnostic-model.md) (rule ids and severities)
* See the [ADR index](index.md).
