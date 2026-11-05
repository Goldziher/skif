# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- Keep a Changelog repeats Added/Changed/Fixed headings per version. -->
<!-- markdownlint-disable MD024 -->

## [Unreleased]

skif has not been released. 0.1.0 will be the first release and will contain everything listed here.

### Added

- **Config model constructors**: every `skif-config` model struct is `#[non_exhaustive]` and builds from `Default` or a
  constructor plus `with_*` methods (`SemverGroup::new`, `DependencyFamily::new`, `VersionGroup`, `IgnoreEntry`,
  `Catalog`, `PluginOptions`, `Workspace`, `Lint`, `Policy`, `WorkspaceProtocol`, `Js`), so `ConfigBuilder::semver_group`
  and `dependency_family` are usable from other crates. `PluginConfig` is `#[non_exhaustive]` too.
- **`Workspace::claimed_dirs`, `Workspace::supports_workspace_protocol` and `Candidates::unreadable`** in
  `skif-workspace`; `WorkspaceError::InvalidYarnrc` for a `.yarnrc.yml` that is not valid YAML.
- **Yarn Berry detection**: a `.yarnrc.yml` or `.yarn/releases` identifies Yarn when nothing stronger does, so a
  checkout without a committed `yarn.lock` is recognised.
- **Why is this used**: `skif explain-usage <FILE[#EXPORT]> [PATH]` prints the entry rule of a file, every import of it
  with file and line, the shortest chain of imports from an entry file, and for an export the modules that use it,
  following re-exports. `FILE` is relative to the current directory; `--format json` prints a stable document;
  `--production`, `--config` and `--no-config` apply as for a lint run; a file outside the module graph exits `2`.
- **`expo-router` and `tanstack-router` plugins**: `app/**` and `src/app/**` are routes of an Expo Router app, unless
  the `root` option of the `expo-router` config plugin names the routes directory, which then replaces them; the
  generated route tree (`tsr.config.json` moves it) and `src/{router,start,client,server}.*` are entries of TanStack
  Router and Start.
- **Yarn catalogs**: `catalog` and `catalogs` in `.yarnrc.yml` (Yarn 4.10+) are read with the other catalog sources, so
  their `catalog:` references resolve instead of being reported by `unresolved-catalog-references`; entries added by
  a fix and `unused-catalog-entries` locate in that file.
- **Namespace export rules**: `unused-namespace-exports` and `unused-namespace-types` report the exports of a module
  that is only read through `import * as ns` or `export * as ns` and that no `ns.member` read touches. A namespace used
  as a whole counts as using everything, and `import * as ns; export { ns }` is a namespace re-export that forwards only
  the members read through it. The findings of such modules move from `unused-exports` and `unused-types` to the new
  rules ([ADR 0029](adrs/0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md)).
- **Single-file component templates**: `.vue`, `.svelte` and `.astro` files credit imports used from the markup
  (component tags, `{{ }}`, `{ }`, `:prop`, `@event`, `v-*`, Svelte blocks and directives), analyse every `<script>`
  block together and the Astro frontmatter, and treat `<style>` blocks and `src` attributes as imports.
- **Plugin virtual modules and linked configs**: `docusaurus` (`@generated/*`, `@theme/*`, the core client API,
  `@site`) and `nuxt` (`#imports`, `#app`, `#components`, `~`, `@`) declare modules the tool resolves itself, `angular`
  links the `tsConfig` and `karmaConfig` of its targets to the `typescript` and `karma` plugins, and `expo`, `metro` and
  `docusaurus` read `babel.config.*`. The new `node` plugin makes `server.js` a production entry and adds the test
  runner's files when a script runs `node --test`; `node --test-reporter` names a package unless it is built in.
- **Karma function-form configs** (`module.exports = function (config) { config.set({ … }) }`) are evaluated.
- **Parity and diff against knip and syncpack**: `mise run parity` compares skif's findings with a pinned knip, and
  `mise run diff:syncpack` compares its policy findings with a pinned syncpack, on the policy fixtures and a pinned
  corpus of 16 public repositories (`skif-bench corpus fetch`, `mise run corpus:fetch`). Counts are committed under
  `tools/bench/baselines`, `--check` fails on drift, and a weekly `corpus` workflow runs it
  ([ADR 0026](adrs/0026-parity-and-diff-tasks-on-a-pinned-corpus.md)).
- **Known deviations in the comparison harness**: `mise run parity` and `mise run diff:syncpack` report intended
  differences from knip and syncpack apart from the one-sided counts (a separate column and section with the reason),
  and `--check` ignores them. syncpack runs with `--dependency-types prod,dev,local` and an empty configuration,
  Rush projects are fed to it as sources and `unresolved-catalog-references` is compared; knip runs with an empty
  configuration (`--repo-knip-config` opts out), enum and namespace members are keyed by member name, noisy stdout
  no longer skips a repository, and `corpus.toml` takes `recursive`, `install-command` and `install-timeout-secs` per
  repository ([ADR 0030](adrs/0030-harness-known-deviations-and-comparison-scope.md)).
- **Benchmark targets are enforced**: `skif-bench run` exits non-zero when an ADR 0001 target fails, measures a
  10,000-file module graph, and `mise run bench:publish` writes `tools/bench/results` and refreshes the numbers table.
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
- **`--no-prompt`**: never ask which alternative fix to apply; a diagnostic with several alternatives stays unfixed
  unless `--select` decides. Both git hooks pass it.
- **`--color <auto|always|never>`**: overrides the terminal check that decides whether human output is coloured.
- **Facade API for layered configuration and several roots**: `discover_targets`, `discover_all`, `find_target`,
  `load_target`, `Target`, `Discovery` and `RootFailure` in `skif`, and `ConfigLoader`, `ResolvedConfig` and `Origins`
  in `skif-config` (re-exported by the facade).
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
- **JS analysis on oxc** with 16 rules: `unused-files`, `unused-dependencies`, `unused-dev-dependencies`,
  `unused-optional-peer-dependencies`, `unlisted-dependencies`, `unlisted-binaries`, `unresolved-imports`,
  `unused-exports`, `unused-types`, `unused-namespace-exports`, `unused-namespace-types`, `unused-enum-members`,
  `unused-namespace-members`, `duplicate-exports`, `circular-imports` and `unparseable-files`. Unused files, exports, types and dev dependencies default to warnings. Production mode,
  `--no-js` and `[js]` settings per workspace are supported.
- **A plugin table of 63 plugins** that reads the static configuration of common tools to find entry files and the
  dependencies they use, without running any tool: `typescript`, `eslint`, `prettier`, `vite`, `vitest`, `jest`,
  `next`, `storybook`, `playwright`, `tailwind`, `postcss`, `babel`, `webpack`, `rspack`, `rollup`, `tsup`, `turbo`,
  `husky`, `lint-staged`, `commitlint`, `changesets`, `github-actions`, `mastra`, `aws-cdk`, `browser-extension`,
  `tsdown`, `oxlint`, `angular`, `karma`, `esbuild`, `svelte`, `nuxt`, `astro`, `drizzle`, `ts-node`, `tsx`, `nx`,
  `cypress`, `mocha`, `sveltekit`, `prisma`, `graphql-codegen`, `docusaurus`, `vitepress`, `wrangler`, `expo`,
  `react-native`, `metro`, `release-it`, `semantic-release`, `lefthook`, `simple-git-hooks`, `stylelint`,
  `markdownlint`, `cspell`, `nodemon`, `pm2`, `pnpm`, `yarn`, `syncpack`, `node`, `expo-router` and
  `tanstack-router`.
  The set is extended in [ADR 0025](adrs/0025-plugin-set-extension-thirty-five-more-tools.md) and ADR 0029.
- **Output formats**: human, JSON, SARIF and GitHub annotations, selected with `--format`.
- **Atomic fixes**: `--fix` and `--fix-unsafe` plan all edits first, write them atomically and lint again;
  edits keep key order, indentation and newline style. `--fix` runs the package manager's install unless
  `--no-install` is given.
- **`skif.toml` configuration** with a committed JSON Schema (`skif schema`), `skif init` and `skif rules`.
- **npm package as a napi-rs addon** for non-EoL Node.js (22, 24, 26) and Bun 1.x on macOS and
  Linux (glibc and musl), plus release
  archives for Windows, `cargo binstall`, Homebrew and Scoop.
- **Git hooks** for pre-commit, Poly and Lefthook, and a GitHub Action (`Goldziher/skif@v0`).
- **Licence notices in every artefact**: release archives, the npm package and each npm platform package carry
  `LICENSE` and `ATTRIBUTIONS.md`; the release workflow and `scripts/check-npm-tarballs.sh` fail without them.
- **Embedding API**: `Executor` is a reusable thread pool a host passes in `RunOptions::with_executor`, so a run
  does not spawn threads of its own. `Linter::with_fs`, `discover_targets`, `find_target` and `find_workspace_root`
  remember directory listings, existence checks and small configuration files for one run (a `Linter` starts every
  run from an empty cache). `FixPlan::base_hash` and `content_hash` let a host refuse to write over a file that
  changed after the fixes were planned, and `skif` does. `ConfigOverrides` has `with_*` builders; `Config`,
  `ConfigOverrides`, the configuration sections, `LintContext` and `skif_lint::RunOptions` are `#[non_exhaustive]`.
  The `skif` crates warn on missing documentation, and CI runs the facade tests and doctests per feature and
  `cargo-semver-checks` (advisory until 0.1.0 is published).
- The documentation shows several roots (`discover_targets`) and layered configuration (`resolve_config`,
  `ConfigLoader`, `ResolvedConfig`) with an example that is compiled as a doctest.

### Changed

- **`workspace-protocol` scope**: the rule is silent for Yarn Classic as it is for npm, skips `peerDependencies`, and
  leaves `file:`/`link:` paths to `local-path-dependency` while that rule is on (so one dependency is reported once).
  `allow-local-paths` decides them only when `local-path-dependency` is off.
- **`incompatible-peer-dependencies` judges by overlap**: a dev range is reported only when no version it allows can
  satisfy the peer range (`^18.0.0` against a peer of `^18.2.0` is fine; `~18.1.0` is not).
- **Catalogs are read for the active package manager**: `pnpm-workspace.yaml` for pnpm (or an unknown manager),
  `.yarnrc.yml` for Yarn, `package.json` always. An invalid `.yarnrc.yml` is reported in `Workspace::manifest_errors`.
- **`skif_workspace::discover_roots` is gone** from the public API (and from `skif`): `discover_targets` is the one
  config-aware pipeline, built on `scan_candidates` and `claim_roots`. A workspace claims a member whose manifest does
  not parse, so that member is no longer linted a second time as a root.
- **`--ignore-package` help** says that the flag replaces `[workspace] ignore-packages` (ADR 0024).
- **`missing_docs`** is a warning in `skif-core`, `skif-specifier` and `skif-workspace` and the whole config model is
  documented (so the JSON Schema gains the descriptions); `skif-specifier` asserts `Send + Sync` for its public types.
- **Alternative fixes carry a typed preference**: `Fix::with_preference(FixPreference::Highest | Lowest)` marks the
  alternative that `[policy] prefer` selects, and `preferred_alternative` and `Alternatives::Preferred` read the tag
  instead of looking for "highest" or "lowest" in the fix title. A fix without a tag is never picked by preference.
- **`RunOptions` and `RenderOptions` are built with `with_*` methods only**: their fields are private, and
  `skif_lint::RunOptions::sequential` is now `with_sequential`.
- **The facade re-exports the configuration model**: `ConfigBuilder`, `Catalog`, `CatalogMode`, `DependencyFamily`,
  `DependencyTypes`, `Js`, `JsWorkspace`, `Lint`, `PluginConfig`, `PluginOptions`, `Policy`, `RuleOptions`,
  `SemverGroup`, `SemverRange`, `VersionGroup`, `VersionPolicy`, `WorkspaceProtocol` and, as `WorkspaceConfig`, the
  `[workspace]` table. The third-party crates that may appear in the API are listed once in the `skif` crate
  documentation and checked by a test ([ADR 0031](adrs/0032-third-party-types-in-the-facade-api.md)).
- **A `Linter` shared by concurrent runs keeps one cache for them**: the cache of the built-in JS analysis is emptied
  when the first of a group of overlapping runs starts and when the last one ends, so `skif -r` no longer wipes the
  listings that the roots already running just read, and an idle `Linter` holds nothing.
- **`skif config -r` walks each tree once** (`unapplied_config_files_all`) instead of once per enclosing root.
- **Internal errors**: a duplicate rule id is reported with kind `internal`, not `config`.
- **Library timings are a hook, not an environment read**: `skif_core::timing` no longer looks at `SKIF_TIMINGS` or
  writes to stderr on its own. A host installs a sink with `timing::enable` (or `enable_stderr`); the `skif` command
  line does so when `SKIF_TIMINGS` is set. The `skif` crate no longer re-exports `timing`.
- **Data types that rules and hosts build are `#[non_exhaustive]`** (`Span`, `Location`, `Related`, `Fix`,
  `TextEdit`, `RuleMeta`, the struct variants of `FixOp` and `JsonAction::Insert`, `Package`, `Manifest`,
  `DependencyEntry`, `FieldSpans`); build them with `Span::new`, `Location::new`, `Related::new`, `Fix::new`,
  `TextEdit::new`, `RuleMeta::new`, `FixOp::{json, yaml, text, create_file, delete_file}` and `JsonAction::insert`.
- **`Workspace` and `Manifest` keep their invariants private**: use `Workspace::{root_package, packages,
  has_root_manifest}` and `Manifest::{source, line_index, value, shared_value}`. `PackageId::from_index` is hidden
  from the documentation.
- **`FileSystem`** gains `read_bytes` and `modified` (with defaults), and `&F`, `Box<F>` and `Arc<F>` are file systems.
- **Faster discovery**: `Workspace::package_by_name` is a hash lookup, each distinct specifier is parsed once per
  workspace, a manifest builds its span table on the first span lookup, and line indexes of ASCII text skip the
  character walk.
- The JSON Schema of `skif.toml` offers `level` (with `severity` as its alias) in a rule table, and the rule
  reference and configuration page document the rule options (`mode` of `member-toolchain-fields`, `ignore-tags` of
  `unused-exports`) and that `unused-catalog-entries` reports only with `[catalog] report-unused-entries = true`.
- **Release hardening**: a release is refused unless the tag is on `main` with a green `ci.yml` run; the Windows
  binaries link the C runtime statically; the aarch64 Windows binary is executed on an ARM64 runner before release;
  the gnu Linux archives document their glibc floor (older systems use the musl archives); `cargo publish` runs a
  workspace dry run before the first upload; the Action's cache key includes the libc; `run-skif.sh` unpacks zips
  without `unzip`; the Action reports a missing release series instead of exiting silently.
- **Rule providers run concurrently** and the catalog analysis is computed once per run (`LintContext::shared`)
  instead of once per catalog rule; the diagnostics are still merged in registration order. A configuration file is
  parsed once, and human and SARIF output find the source line of a diagnostic without rescanning the file.
- The `policy` feature of the `skif` crate is removed (the policy rules always shipped with `manifest`);
  `serde_json` is a dependency of `skif` only with the `schema` feature. `RunOptions` of `skif-lint` is built with
  `with_*` methods and takes an `Executor` instead of a rayon pool.
- Minimum supported versions raised: Rust 1.99.
- **The project is renamed to skif.** It continues as an independent Rust dependency linter for
  JavaScript and TypeScript monorepos, hosted at `github.com/Goldziher/skif`.
- **The code is a library-first workspace** of `skif-*` crates with a thin `skif` command line on top.
  Architecture decisions are recorded in `adrs/`.

### Fixed

- **Root discovery reads manifests like the loader**: a root `package.json` with a byte order mark, comments or
  trailing commas that declares `workspaces` is a workspace root for `-r`, and `declared_member_dirs` reads it too.
- **`[workspace] discover-ignore` is validated**: an invalid pattern is a configuration error naming
  `workspace.discover-ignore[i]` instead of being skipped.
- **One unreadable directory entry no longer hides its siblings** in a listing; a directory that cannot be listed is
  reported in `Candidates::unreadable`.
- **A `package.json` section written twice** keeps the entries of its last occurrence only, as the JSON value does.
- **`same-range` on a dependency shared by many packages** judges each distinct specifier once, and the `*`/`latest`/
  `workspace:*` range is parsed once.
- **Facade, CLI and addon** (audit findings): `Target::config_path` names a configuration file outside the root's
  ancestors (`--config /elsewhere/skif.toml`) with `..` segments instead of returning nothing; the stale-file guard
  also covers deleted files and the CLI refuses to write a file that changed after planning (tested);
  `find_workspace_root` and `find_target` share one ancestor search; the sync `lint` of the npm addon throws an
  `Error` with `kind` and `path`; the JSON document of a recursive run lists `truncated` directories; the CLI
  reference test matches whole flags; SARIF writes a Windows drive as `file:///C:/...`; the GitHub format says when
  more annotations are emitted than GitHub shows; a temporary file next to a target no longer repeats the target's
  name, so a name at the length limit can be fixed; package-scoped `[[overrides]]` are resolved per package once
  instead of against every diagnostic; `Through` forwards `read_bytes` and `modified`.
- **Knip parity in the JS analysis** (found by running both tools on the pinned corpus): `unused-enum-members` and
  `unused-namespace-members` now count a member read in the declaring file by name, and no whole-enum use there
  (`Object.values(E)`, `E[key]`, a spread, a bare reference) credits a member;
  the types in a destructured export (`export const { a, b } = make((p: Props) => p)`) belong to it; a file loaded with
  `new URL("./x", import.meta.url)` is used as a whole even when the file also imports names from it; `require("x") as typeof import("x")` no longer uses every export;
  `export const X` with `export type X` is reported once; `*` in a wildcard `exports` target matches across directories;
  `duplicate-exports` ignores export clauses and follows `export const b = a` (not `a as T` or `a!`); only the `exports` field is widened
  (`./*` is skipped, `main`, `bin` and `types` keep their single-directory globs).
- **Catalog ownership with `mode = "off"`**: an instance whose sibling uses `catalog:` was reported by both
  `catalog-mismatch` and `version-mismatch` with conflicting fix targets. `catalog-mismatch` now owns it and the version
  groups leave it alone while that rule is enabled.
- `same-range` and `same-minor` version groups no longer flag `workspace:` dependents of a local package.
- A `snapped-to` group takes the snap package's production dependency as the target, not a peer range written as a
  compatibility window; a catalog entry created from several specifiers takes the one with the highest lowest version,
  as version groups do; a `link:` path that climbs out of the workspace never names a local package.
- An override pattern `./dir/**` also selects the package located at `dir`.
- A configuration ceiling that is not above the root, or a relative root, is an error instead of silently reading no
  configuration; `.` and `..` in a root no longer make the same file be read twice.
- `"workspaces": []`, `null` and `false` in a `package.json` no longer make the directory a workspace; Rush
  `projectFolder` values are matched literally; linked workspace members are resolved through the file system in use.
- Origins of keys that contain a dot (`js.workspaces."pkg/a.b"`) no longer collide with nested tables.
- **JS analysis, ancestors and nesting**: a dependency listed by a `package.json` above a package (the package that
  contains it, the root, or a manifest above a nested root up to the git root) counts as listed for its imports and
  enables the plugins of its tools; the listing manifest's dependency is used, not unused. Entry globs that plugins
  derive from the root configuration (a Playwright `testDir`, a Jest `testMatch`) match files of member packages.
- **JS analysis, build output**: `lib-commonjs`, `lib-esm`, `dist-cjs`, `esm5` and similar output directories map back
  to `src`; extension-less `#!/usr/bin/env node` bin stubs are read for the sources they load; a package whose
  published entry points have no source behind them is not judged for unused files and dependencies.
- **JS analysis, configuration**: `tsconfig` `extends` follows workspace and installed packages and `${configDir}` is
  expanded; tsconfig files are parsed once; `projects` of Jest and Vitest given as directories, globs or config
  files are read; `.vue`, `.svelte` and `.astro` files are project files; `jsxImportSource` in a `tsconfig` stops JSX
  files from crediting `react`; `src/out`, `lib/target` and `coverage` below a package root are source folders.
- **Scripts**: `"#note"` and `"//note"` keys are comments, `break` and `continue` are shell builtins, and `bun <word>`
  no longer reports the word as a binary. A binary named like a listed package that is not installed
  (`docusaurus-start` from `docusaurus`) is not reported.
- **Config evaluation and stylesheets**: URLs with a scheme and `.href` are unknown paths; `path.posix`, `import * as
  path` and `import { default as path }` are understood; `.pcss` files take `//` comments; an apostrophe in an unquoted
  `url()` no longer swallows the next `@import`; `tsconfig` `paths` with an inner wildcard are not followed as
  directories; browser-extension `chrome_url_overrides`, `sandbox.pages`, `sidebar_action` and `background.page` are
  pages; `%PUBLIC_URL%` and template placeholders in HTML script tags are handled.
