# skif commands and flags

The full reference, with every edge case, is <https://goldziher.github.io/skif/reference/cli/>. `skif <command>
--help` prints the flags of the installed version.

## Install

| Channel | Command |
| --- | --- |
| npm, pnpm, yarn, bun | `pnpm add -D skif-cli` (or `npm install -D`, `yarn add -D`, `bun add -D`); run once with `npx skif-cli`, `pnpm dlx skif-cli`, `yarn dlx skif-cli` or `bunx -p skif-cli skif` |
| Homebrew | `brew install goldziher/tap/skif` |
| Scoop | `scoop bucket add goldziher https://github.com/Goldziher/scoop-bucket`, then `scoop install skif` |
| Cargo | `cargo install skif-cli` or `cargo binstall skif-cli` (the `skif` crate is the library and has no binary) |

The npm package is named `skif-cli`; `npx skif` is a different package.

## Which project is linted

`skif [PATH]` lints the nearest workspace that lists the package at `PATH` as a member, else that package alone.
With `-r`/`--recursive`, or when no `package.json` is at or above `PATH`, every workspace and standalone package
below `PATH` is linted, each with its own configuration, into one report and one exit code. Reported paths are
relative to the directory skif was started in.

## Lint flags

Output:

| Flag | Meaning |
| --- | --- |
| `--format <FORMAT[=FILE]>` | `human`, `json`, `sarif`, `github`, `codeclimate`, `compact`, `markdown`, `disclosure`, `codeowners`. Repeatable; at most one goes to stdout, the others to `FILE`. |
| `--output <FILE>` | Write the single `--format` to a file. |
| `--max-show-issues <N>` | Show at most `N` findings per rule (human and json only); the summary counts all. |
| `--max-warnings <N>`, `--fail-on-warnings` | Exit `1` above `N` warnings / on any warning. |
| `--max-issues <N>` | Exit `1` only above `N` errors. |
| `--no-exit-code` | Exit `0` when problems remain (`2` is kept). |

Scope of the analysis:

| Flag | Meaning |
| --- | --- |
| `-r`, `--recursive` | Every root at or below `PATH`. |
| `-W`, `--workspace <NAME\|DIR>` | Report only these packages (`.` is the root); the analysis still covers all. |
| `--include`, `--exclude <TYPE\|RULE>` | Only / not these rule IDs or knip issue types (`files`, `dependencies`, `exports`, `types`, `unlisted`, ...). |
| `--dependencies`, `--exports`, `--files`, `--cycles` | knip's shortcuts for `--include`. |
| `--no-js` | Skip the JavaScript analysis (manifest, workspace and policy rules only; what the git hooks run). |
| `--production` | Analyse production code only. |
| `--strict` | knip's strict mode: `--production`, and each package may use only what its own manifest lists. |
| `--include-entry-exports` | Also report unused exports of entry files. |
| `--tsconfig <FILE>`, `--use-tsconfig-files` | TypeScript configuration of the root / take project files from `tsconfig.json`. |
| `--no-gitignore` | Also analyse files `.gitignore` hides. |
| `--tags <-TAG>` | Leave out exports tagged `@TAG`. |
| `--dependency`, `--dependency-types`, `--specifier-types`, `--source` | Narrow the version-group and semver-group rules to some instances. |

Configuration from the command line (the top layer, above the files):

| Flag | Meaning |
| --- | --- |
| `--rule <ID>=<off\|info\|warn\|error>` | Set a rule's level. |
| `--ignore-rule <ID>` | Turn a rule off. |
| `--ignore-package <NAME>` | Ignore a package in every rule (replaces the file's list). |
| `--ignore-dependency <NAME>` | Ignore a dependency in every rule. |
| `--ignore-rule-for-package <RULE:PACKAGE>` | Ignore one rule for one package. |
| `--require-ignore-reasons` | Exit `2` when an ignore entry of a configuration file has no `reason`. |
| `--config <FILE>`, `--no-config` | Use this file instead of discovery / read no file. |

Fixing:

| Flag | Meaning |
| --- | --- |
| `--fix` | Apply safe fixes, lint again, run the package manager's install. |
| `--fix-unsafe` | Also apply unsafe fixes; implies `--fix`. |
| `--dry-run` | With a fix flag: print the planned edits as a diff; write nothing, install nothing. |
| `--allow-remove-files` | Let the `unused-files` fix delete files. |
| `--fix-type <TYPE>` | Only the fixes of these rule IDs or knip issue types (`exports`, `types`, `dependencies`, `files`, `catalog`, ...). |
| `--select <highest\|lowest>` | Which version a version alignment targets. |
| `--no-install` | Skip the install step. |
| `--no-prompt` | Never ask which alternative fix to apply. |

Diagnosing: `--trace-dependency <NAME>` (where a dependency is listed, imported and counted as used), `--debug`
(roots, configuration files and packages loaded), `--performance`, `--duration`, `--memory`. Running: `--watch` (lint
again on every change), `--cache` and `--cache-location <DIR>` (keep parse results between runs; never changes the
report), `--color <auto|always|never>`.

## Subcommands

| Command | Notes |
| --- | --- |
| `skif rules [--format json]` | Every rule with `id`, `category`, `default`, `fixable`, `summary`. |
| `skif schema` | The JSON Schema of `skif.toml`. |
| `skif init [PATH]` | Write a starter `skif.toml` with a `#:schema` directive; refuses to overwrite. |
| `skif config [PATH] [-r] --show` | The files that apply, in order, then the merged configuration. |
| `skif config [PATH] --explain <KEY> [--package <NAME>]` | Every assignment of a key (`rules.unused-exports.level`, `js.ignore-dependencies`, `version-groups[0]`) and its final value. |
| `skif explain-usage <FILE[#EXPORT]> [PATH] [--production]` | Why a file or export is used: entry rule, importers, the import chain from an entry, a verdict for an export. |
| `skif migrate [PATH] [--from sherif,syncpack,knip\|auto] [--profile exact\|full] [--merge\|--force] [--dry-run] [--strict]` | See [migrate.md](migrate.md). |
| `skif baseline write\|prune [PATH] [-r\|--file <FILE>] [LINT FLAGS] [--reason <TEXT>]` | Record / prune findings as `[[ignore]]` entries in `skif.baseline.toml`. |
| `skif baseline check [PATH] (--base <REF>\|--base-file <FILE>)` | Exit `1` when the baseline gained entries. |
| `skif update [PATH] [--check\|--interactive] [--target latest\|minor\|patch] [--dependencies <NAME>]` | Newer published versions from the npm registry; the only command that uses the network. |
| `skif list [PATH] [-r] [--show ...]`, `skif json [PATH] [-r]` | The dependencies the version groups govern, as text / NDJSON. |
| `skif format [PATH] [-r] [--check\|--unsafe]` | Format every `package.json` as `[format]` describes. |
| `skif mcp` | MCP server on stdio with the tools `lint`, `explain_usage`, `list_rules`; it never writes files. |
| `skif lsp` | Language server on stdio: diagnostics as you edit, fixes as code actions. |

`rules`, `config`, `explain-usage`, `migrate`, `baseline` and `update` take `--format human|json`.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Clean (warnings within `--max-warnings` allowed; `info` never counts). |
| `1` | Problems remain; for `baseline check`, the baseline gained entries. |
| `2` | skif could not run: invalid configuration, no `package.json` at or below `PATH`, bad flags, a refused fix, a failed install step. |
