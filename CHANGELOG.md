# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- Keep a Changelog repeats Added/Changed/Fixed headings per version. -->
<!-- markdownlint-disable MD024 -->

## [Unreleased]

skif has not been released. 0.1.0 will be the first release and will contain everything listed here.

### Added

- **Several projects in one run**: `-r`/`--recursive` lints every workspace and standalone package at or below the
  start path, and so does a run started where neither the path nor an ancestor holds a `package.json`. Results are
  merged into one report with one exit code; `--format json` adds a `roots` array. Each root merges the
  `skif.toml` files from its directory up to the git repository root. `root-package-manager-field` does not report for a standalone
  package below another root.
- **Layered configuration**: every `skif.toml` from a root up to the git repository root merges, nearest last, so
  an organisation file at the top applies to every project below it. `root = true` stops the search, `extends`
  includes preset files, `reset` drops inherited lists, and `extend-` keys append to ignore lists. `[[overrides]]`
  scope rule levels, `max-level`, suppressions and JS inputs to packages or files; a `skif.toml` in a workspace
  member directory does the same for that package; `$ROOT/` anchors a pattern at the root being linted. `skif
  config --show` and `--explain` print the effective configuration with the file and line of every value, and
  `--config` and `--no-config` bypass discovery
  ([ADR 0024](adrs/0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md)).
- **Workspaces without a root `package.json`**: a directory with a `pnpm-workspace.yaml` or a `rush.json` and no
  `package.json` is linted as a workspace, with its members linted and the rules about the root manifest skipped,
  instead of failing the run.
- **Rush**: `rush.json` (JSONC) declares a workspace. `projects[].projectFolder` are the members and
  `pnpmVersion`, `npmVersion` or `yarnVersion` name the package manager.
- **`[workspace] discover-ignore`**: directories a recursive run does not search for roots. The default is
  `__fixtures__`, `fixtures`, `__mocks__` and `__snapshots__`; setting the key replaces the list
  ([ADR 0023](adrs/0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md)).
- **Single-package mode**: a package that declares no workspace is linted on its own, with no workspace required.
  `root-package-private-field` and `root-package-dependencies` do not report for it.
- **`recursive` option** for `lint` and `lintAsync` in the npm addon.
- **`skif config`**: `skif config [PATH] [-r] --show` prints the configuration files a root is built from, in the
  order they apply (root, ancestor, extended by, explicit), and the effective merged configuration as TOML or, with
  `--format json`, JSON. `--explain <KEY>` lists every assignment of a key (`rules.ID.level`, `js.ignore-dependencies`,
  `version-groups[0]`) with file, line, column and operation, and its final value; `--package <NAME>` adds the
  overrides that apply to that package. `skif init` names the files the new file will inherit from.
- **`--config <FILE>` and `--no-config`**: use one configuration file (and what it extends) instead of discovering
  `skif.toml` files, or read none. The npm addon gains the `config` and `noConfig` options.
- **One pass over the file system**: a run reads each directory and each configuration file at most once; one caching
  file system is shared by root discovery, workspace discovery, plugin file searches, JS discovery and the module
  resolver, and JS discovery walks sibling directories in parallel.
- `SKIF_TIMINGS=1` prints the duration of each phase of a run (root discovery, workspace discovery, configuration,
  every rule provider, JS discovery, parsing, resolution and graph building, rendering) to stderr.
- **Manifest and workspace rules** with stable ids, severities and autofixes: `empty-dependencies`,
  `incompatible-peer-dependencies`, `local-path-dependency`, `member-toolchain-fields`,
  `missing-package-manifest`, `root-package-dependencies`, `root-package-manager-field`,
  `root-package-private-field`, `types-in-dependencies`, `unmatched-workspace-patterns` and
  `unsorted-dependencies`.
- **Version, semver-range and catalog policies**: version groups, semver groups, banned dependencies,
  dependency families, workspace-protocol and catalog rules, with catalog resolution for pnpm, package.json
  and bun catalogs and recursive workspace globs (`**`) with pnpm semantics.
- **JS analysis on oxc** with 14 rules: `unused-files`, `unused-dependencies`, `unused-dev-dependencies`,
  `unused-optional-peer-dependencies`, `unlisted-dependencies`, `unlisted-binaries`, `unresolved-imports`,
  `unused-exports`, `unused-types`, `unused-enum-members`, `unused-namespace-members`, `duplicate-exports`,
  `circular-imports` and `unparseable-files`. Unused files, exports, types and dev dependencies default to warnings. Production mode,
  `--no-js` and `[js]` settings per workspace are supported.
- **A plugin table of 25 plugins** that reads the static configuration of common tools to find entry files and the
  dependencies they use, without running any tool: `typescript`, `eslint`, `prettier`, `vite`, `vitest`, `jest`,
  `next`, `storybook`, `playwright`, `tailwind`, `postcss`, `babel`, `webpack`, `rspack`, `rollup`, `tsup`, `turbo`,
  `husky`, `lint-staged`, `commitlint`, `changesets`, `github-actions`, `mastra`, `aws-cdk` and `browser-extension`.
- **Output formats**: human, JSON, SARIF and GitHub annotations, selected with `--format`.
- **Atomic fixes**: `--fix` and `--fix-unsafe` plan all edits first, write them atomically and lint again;
  edits keep key order, indentation and newline style. `--fix` runs the package manager's install unless
  `--no-install` is given.
- **`skif.toml` configuration** with a committed JSON Schema (`skif schema`), `skif init` and `skif rules`.
- **npm package as a napi-rs addon** for Node 24+ and Bun on macOS and Linux (glibc and musl), plus release
  archives for Windows, `cargo binstall`, Homebrew and Scoop.
- **Git hooks** for pre-commit, Poly and Lefthook, and a GitHub Action (`Goldziher/skif@v0`).

### Changed

- Minimum supported versions raised: Rust 1.99.
- **The project is renamed to skif.** It continues as an independent Rust dependency linter for
  JavaScript and TypeScript monorepos, hosted at `github.com/Goldziher/skif`.
- **The code is a library-first workspace** of `skif-*` crates with a thin `skif` command line on top.
  Architecture decisions are recorded in `adrs/`.
