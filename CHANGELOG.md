# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- Keep a Changelog repeats Added/Changed/Fixed headings per version. -->
<!-- markdownlint-disable MD024 -->

## [Unreleased]

### Added

- knip's run-time filters on the lint command ([#36](https://github.com/Goldziher/skif/issues/36)):
  - `--workspace <NAME|DIR>` (`-W`) reports only the findings of the selected workspace packages; findings of no
    package belong to the root.
  - `--strict` is `--production` with each workspace package limited to the dependencies its own manifest lists.
  - `--include` and `--exclude` take knip issue types or skif rule ids, with the shortcuts `--dependencies`,
    `--exports`, `--files` and `--cycles`; `--include` turns on a rule that is off; `--tags -TAG` leaves out tagged
    exports.
  - `--max-issues <N>`, `--max-show-issues <N>` (human and JSON output, not with `--fix`), `--no-exit-code` and
    `--include-entry-exports`.
  - `--no-gitignore` analyses the files `.gitignore` hides.
  - `--trace-dependency <NAME>` prints where a dependency is listed, imported and used.
  - `--debug`, `--performance`, `--duration` and `--memory` print what was loaded, phase timings, wall time and peak
    memory to stderr.
- An `unplugin-icons` plugin: with that dependency, `~icons/*` imports and the `@iconify-json/*` and `@iconify/json`
  collections are virtual, as in knip. The table holds 106 plugins
  ([#18](https://github.com/Goldziher/skif/issues/18)).
- `--allow-remove-files` lets a fix run delete files, and `--fix-type` limits it to some rules, given as rule ids or
  knip issue types (`dependencies`, `exports`, `types`, `enumMembers`, `files`, `catalog`, ...), as knip's flags do
  ([ADR 0052](adrs/0052-file-deletion-opt-in-fix-types-and-member-removal-fixes.md),
  [#37](https://github.com/Goldziher/skif/issues/37)).
- Unsafe fixes for `unused-enum-members`, `unused-namespace-members` and `unused-catalog-entries`, and for
  `unused-exports` on `export default <expression>` when the expression has no side effects. A member goes with its
  comments; a member whose removal would renumber a later enum member gets no fix; a catalog left empty is removed
  ([ADR 0052](adrs/0052-file-deletion-opt-in-fix-types-and-member-removal-fixes.md),
  [#37](https://github.com/Goldziher/skif/issues/37)).
- `skif mcp`, a Model Context Protocol server on stdio for AI agents, with the tools `lint`, `explain_usage` and
  `list_rules`, which return the documents of `--format json`
  ([ADR 0056](adrs/0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md),
  [#44](https://github.com/Goldziher/skif/issues/44)).
- `--watch` lints again whenever a file below the root changes, after 100 milliseconds of quiet; it polls, so it
  needs no native file events, and ignores the reports it writes (`--output`, `--format FORMAT=FILE`)
  ([ADR 0056](adrs/0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md),
  [#44](https://github.com/Goldziher/skif/issues/44)).
- `skif lsp`, a minimal language server on stdio that publishes skif's diagnostics when a file is opened or saved
  ([ADR 0056](adrs/0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md),
  [#44](https://github.com/Goldziher/skif/issues/44)).

### Changed

- `--fix-unsafe` no longer deletes files: the `unused-files` fix is skipped as `removes-files` unless
  `--allow-remove-files` is given. In the library, `FixSelection::remove_files` is off by default and such a fix is
  skipped with `SkipReason::RemovesFiles`
  ([ADR 0052](adrs/0052-file-deletion-opt-in-fix-types-and-member-removal-fixes.md),
  [#37](https://github.com/Goldziher/skif/issues/37)).
- Files in `fixtures` and `__fixtures__` directories are no longer reported by `unused-files`, and their exports,
  types and members are no longer reported by `unused-exports`, `unused-types`, the namespace rules and
  `unused-enum-members`. `[js] include-fixtures = true` reports them like any other file
  ([ADR 0050](adrs/0050-fixture-directories-in-the-js-analysis.md),
  [#28](https://github.com/Goldziher/skif/issues/28)).
- `--fix` aligns versions when the target is explicit (a `pinned` group's version, the version the workspace aligns
  on, an existing catalog entry) and the move stays within what the specifier already promises, in the same shape: an
  exact version or a `^` range within its major (`left-pad` `1.0.0` to `1.3.0`), a `~` range within its minor.
  Moves across a major, downgrades, prereleases and range changes still need `--fix-unsafe`, also when a pin asks
  for them
  ([ADR 0051](adrs/0051-safe-version-alignment-fixes.md), [#31](https://github.com/Goldziher/skif/issues/31)).
- **Breaking** for consumers of `explain-usage --format json`: the `absence` value `not-analysed` is gone, replaced
  by `package-excluded`, `declaration-file`, `entries-incomplete`, `outside-project`, `ignored` and `not-discovered`
  ([#23](https://github.com/Goldziher/skif/issues/23)).
- Module resolution takes about half the thread time it did on large monorepos (n8n, cal-com), and a full run
  15 to 20% less wall time, with identical output. The file system cache hashes the bytes of a path instead of its
  components and finds a name in a cached listing by its name; the resolver looks up the scopes of an importer once,
  keeps the source file of a workspace package per specifier, and no longer reads and parses a `tsconfig.json` that
  cannot be loaded (its `extends` names a package that is not installed) again for every importer below it
  ([#30](https://github.com/Goldziher/skif/issues/30)).
- `skif migrate` accepts knip's `compilers: true` and `true` for an extension skif reads itself (`mdx`, `vue`,
  `svelte`, `astro`, the stylesheet languages, `tsrx`) as the built-in extractors, instead of reporting it as
  unsupported; a custom compiler function for one of them is an approximation, since it never runs
  ([#38](https://github.com/Goldziher/skif/issues/38)).

### Fixed

- `unused-namespace-members` counts a member read by its bare name inside the namespace body, as a value or as a
  type, unless a parameter or local of the same name shadows it; the declarations of a merged namespace share their
  members ([#15](https://github.com/Goldziher/skif/issues/15)).
- `unused-dependencies` no longer reports `optionalDependencies`, as knip does not: they are mostly platform packages
  loaded by a computed name ([#21](https://github.com/Goldziher/skif/issues/21)).
- `skif migrate` writes `[catalog] report-unused-entries = true` when knip's `catalog` issue type stays enabled, so
  `unused-catalog-entries` keeps reporting what knip reported ([#32](https://github.com/Goldziher/skif/issues/32)).
- `skif -r <rule>` with a sherif or skif rule name, where sherif's `-r` is `--ignore-rule`, now fails with a usage error
  that names `--ignore-rule` and `--rule` for that rule, instead of a missing-path error
  ([#45](https://github.com/Goldziher/skif/issues/45)).
- `unused-exports`, `unused-types`, the namespace export rules and the member rules no longer report exports of a
  package whose entry files are incomplete, as `unused-files` and `unused-dependencies` already did: a file no known
  entry reaches may import them. `explain-usage` gives such an export the verdict `not-judged`
  ([#12](https://github.com/Goldziher/skif/issues/12)).
- A relative import of a package directory (`..`, `../../automation`) whose `main` or `exports` point at build output
  that was never built resolves to the source file behind it, as an alias to such a directory already did, instead of
  being reported by `unresolved-imports` ([#17](https://github.com/Goldziher/skif/issues/17)).
- `unlisted-dependencies` no longer reports packages that a test only mocks (`vi.mock("x")`, `jest.mock("x")`),
  and mock calls are read from the syntax tree, so module names inside strings and template literals are no longer
  imports; `{ virtual: true }` mocks are ignored ([#16](https://github.com/Goldziher/skif/issues/16)).
- Imports inside `declare module "x" { ... }` bodies now count as uses of the packages they name, so those
  packages are no longer reported as unused ([#20](https://github.com/Goldziher/skif/issues/20)).
- `declare module "x"` in a file with top-level `import` or `export` augments `x` instead of declaring it, so it no
  longer hides unlisted imports of `x` across the workspace; only declarations in script files do
  ([#24](https://github.com/Goldziher/skif/issues/24)).

Repositories with nested roots ([#19](https://github.com/Goldziher/skif/issues/19)):

- A file of a nested project (a directory with its own `package.json`, such as a nested root) that the outer root
  reaches is not reported in the outer root: its imports are no longer unlisted there and no longer count as uses of
  the outer root's dependencies. The nested root judges the file against its own manifest in its own run.
- A file that a script of a `package.json` above a nested root runs (`node build/npm/postinstall.ts`) is an entry
  of that root, so it and what it imports are no longer reported as unused files.
- A package with no source file, no entry and no script no longer reports every dependency it lists as unused: there
  is no code to judge them against.
- `stale-js-config` no longer stays silent about every `ignore-dependencies` list when one package has entry files
  skif cannot determine: the lists are judged over the other packages, and only a global entry matching a dependency
  of such a package is spared.
- A bare `typeof import("./m")`, or a bare JSDoc `@type {import("./m")}`, no longer marks every export of `./m`
  used: it names the module's type without reading an export, as in knip. `typeof import("./m").A` uses `A`, and
  `typeof import("./m").E.B` only member `B` of `E`; the indexed `typeof import("./m")["A"]` that unplugin-auto-import generates uses `A` ([#25](https://github.com/Goldziher/skif/issues/25)).
- `unresolved-imports` no longer reports `./dir` when a `.gitignore` lists `dir/index.ts` (or an extensionless
  `./file` when it lists `file.ts`), nor a `#` specifier of a package whose nearest `package.json` declares no
  `imports`, unless a `tsconfig` `paths` pattern or a configured alias maps it ([#18](https://github.com/Goldziher/skif/issues/18)).
- The `provider` of a Prisma schema's `generator` block (`prisma-kysely`, `zod-prisma-types`) counts as used
  ([#22](https://github.com/Goldziher/skif/issues/22)).
- `import.meta.resolve("x")` counts as a use of `x`, like `require.resolve`
  ([#22](https://github.com/Goldziher/skif/issues/22)).
- A file a script runs through a path that leaves its package (`tsx ../../scripts/seed.ts`) is the file it names and
  belongs to the package it lies in, and `explain-usage` gives such a file, or one named `./scripts/a.ts`, as run by a
  script ([#22](https://github.com/Goldziher/skif/issues/22)).
- The script parser reads `$((n+1))` and `((n++))` as arithmetic, not as commands; `bun --cwd <dir>` and
  `pnpm --prefix <dir>` (like `--dir` and `-C`) run in another package, so the scripts and binaries they name are that
  package's ([#22](https://github.com/Goldziher/skif/issues/22)).
- `new URL("../node_modules/.bin/tsx", import.meta.url)` and `new URL("./node_modules/later", import.meta.url)` name
  installed files, not modules, so they no longer report `.bin` or `later` as unlisted
  ([#22](https://github.com/Goldziher/skif/issues/22)).
- A `tsconfig` with `outDir` and no `rootDir` maps its build output back to source through the common directory of
  its `include` and `files`, and script arguments inside an `outDir` map to their sources, ahead of build output on
  disk: `mocha --recursive ./build/compiled/test` makes every test file below `test/` an entry (without
  `--recursive` only those directly in it, with mocha's `--extension` values), so compiled test suites are no longer
  reported as unused files with unused dev dependencies ([#13](https://github.com/Goldziher/skif/issues/13)).
- `explain-usage` says why a file outside the module graph is not reported: the package is excluded, a declaration
  file, ignored, outside the project globs, not discovered, or in a package whose entry files are not fully known. For
  the last it names every tool config that could not be read and why (it does not parse, it evaluates to an unknown
  value, it spreads in an unknown base config, its entry keys are computed, or `package.json` publishes build output
  with no source behind it). The JSON report's `incomplete` lists the configs
  ([#23](https://github.com/Goldziher/skif/issues/23)).
- The JSON report lists under `incomplete-packages` the packages whose unused files and dependencies were not checked
  because their entry files are not all known, with each reason (plugin, config file, cause); human output names them
  in a note before the summary. `Report` carries them as `incomplete_packages`. `skif-bench parity` counts knip-only
  files and dependencies in those packages as `not-judged-by-skif`, apart from agreement and the other deviations
  ([#27](https://github.com/Goldziher/skif/issues/27)).
- The files an n8n node package lists in `package.json` `n8n.nodes` and `n8n.credentials` are entries, mapped from
  `dist` back to their sources, so node and credential files are not reported unused
  ([#23](https://github.com/Goldziher/skif/issues/23)).
- `--format codeclimate` (GitLab Code Quality JSON with stable fingerprints), `compact` (one
  `path:line:col rule message` line per finding), `markdown` and `disclosure` (a table per rule for pull request
  comments, folded in `<details>` sections for `disclosure`) and `codeowners` (findings grouped by the owners in the
  repository's CODEOWNERS file, last matching pattern wins, GitLab section default owners apply). The renderers are
  in `skif-report` and `skif::render` ([#42](https://github.com/Goldziher/skif/issues/42)).
- `--format` is repeatable and takes `FORMAT=FILE`, so one run writes several reports, each to its own file, with at
  most one format on stdout. Files are written only after every rendering succeeds, though a write that fails
  partway can leave earlier files written; `--output` stays the single-format form ([ADR 0054](adrs/0054-several-reports-in-one-run.md),
  [#42](https://github.com/Goldziher/skif/issues/42)).
- The `include-tags` option of `unused-exports` reports only exports and members carrying one of its tags, like knip's
  `--tags +name`; `skif migrate` translates `+tag` to it. A tag it names is judged even when it would exempt, so
  `include-tags = ["public"]` reports unused `@public` exports (knip reports none). `stale-js-config` reports knip's tag hints: an
  `ignore-tags` tag, or `@internal` in production mode, on an export or member that is used anyway
  ([#43](https://github.com/Goldziher/skif/issues/43)).
- `stale-js-config` reports `entry`, `project` and `ignore` patterns that match no file, known to the analysis or on
  disk (gitignored and non-source files count), like knip's `entry-empty`, `project-empty` and `ignore` configuration
  hints; a pattern of a glob `[js.workspaces]` key is reported once, under the key as written, when it matches in none
  of its packages ([#43](https://github.com/Goldziher/skif/issues/43)).
- `[js.cycles]` with `allow` (cycles not reported; an empty cycle or path is a configuration error) and
  `dynamic-imports` (count `import()` in `circular-imports`), translated by `skif migrate` from knip's `cycles`
  ([#43](https://github.com/Goldziher/skif/issues/43)).
- `[js] ignore-files`, top level and per workspace: files `unused-files` never reports, which other rules still analyse.
  `skif migrate` writes knip's `ignoreFiles` there instead of widening it to `ignore`, and `stale-js-config` reports an
  entry that hides nothing ([#43](https://github.com/Goldziher/skif/issues/43)).
- An export tagged `@alias` is no longer reported by `unused-exports`, `unused-types` or `duplicate-exports`, as in
  knip ([#43](https://github.com/Goldziher/skif/issues/43)).
- `skif migrate` no longer says skif has no tag hints or global binaries list: `treatTagHintsAsErrors` points at
  `stale-js-config`, and `ignoreGlobalBinaries: true` is noted as what skif always does
  ([#43](https://github.com/Goldziher/skif/issues/43)).
- An enum or namespace member tagged `@internal` (with `production`), `@beta`, `@alpha`, `@alias` or an `ignore-tags`
  name is no longer reported by `unused-enum-members` or `unused-namespace-members`; only `@public` was honoured
  ([#43](https://github.com/Goldziher/skif/issues/43)).
- `package.json` formatting with syncpack's `format` semantics: the `package-json-format` rule (off by default),
  `skif format`, `skif format --check` and `skif format --unsafe`, and a `[format]` table with `sort-first`, `sort-az`,
  `sort-exports`, `sort-packages`, `format-bugs`, `format-repository` and `indent`. By default only `types` moves first
  and `default` last among `exports` conditions, since the others decide resolution; a longer `sort-exports` gives an
  unsafe fix. Comments, negated globs and non-git repositories are kept. `skif migrate` writes syncpack's format
  options to it ([ADR 0055](adrs/0055-package-json-formatting-the-package-json-format-rule-and-skif-format.md),
  [#33](https://github.com/Goldziher/skif/issues/33)).
- `.mdx` files are followed: their top-level `import` and `export` statements are analysed and the components their
  prose names count as uses (code in backticks does not), so the components, packages and `@mdx-js/*` compilers of a
  docs site are no longer unused. An apostrophe in JSX text or a multi-line template literal does not make an `export`
  paragraph swallow the rest of the document ([#38](https://github.com/Goldziher/skif/issues/38)).
- The `templateUrl`, `styleUrl` and `styleUrls` of an Angular or Stencil `@Component` are followed, so the packages
  its stylesheets import are no longer unused ([#38](https://github.com/Goldziher/skif/issues/38)).
- Stylus files (`.styl`, `.stylus`, `<style lang="stylus">`) are followed through `@import` and `@require`, and the
  imports of `.tsrx` files are read (not those in comments, strings or template literals, and not re-exports), so the
  files and packages they use and the `stylus` and `@tsrx/*` compilers are no longer unused
  ([#38](https://github.com/Goldziher/skif/issues/38)).

## [0.1.1] - 2026-11-07

### Changed

- The README leads with what skif does and how it compares, and publishes measured speed, seeded-defect accuracy and
  agreement with knip.
- The comparison with knip counts the same export under another label as agreement, and reports paths knip does not
  read as a known deviation ([ADR 0049](adrs/0049-js-corrections-from-adjudicating-knip-disagreements.md)).
- The oxc crates move from 0.156.0 to 0.157.0, and use caret ranges instead of exact pins.

### Added

- `tap`, `oxfmt` and `bun` plugins. The table holds 105 plugins.

### Fixed

False findings on real repositories ([ADR 0049](adrs/0049-js-corrections-from-adjudicating-knip-disagreements.md)):

- A symbolic link to a source file is that file: its exports are no longer reported once per link, and its target no
  longer looks unused.
- Every condition of a `package.json` `imports` entry is reachable, so a file only `browser` selects is not unused.
- A package reached through a `tsconfig` path alias, or augmented with `declare module`, counts as used when the
  manifest lists it.
- ``require(`./commands/${name}`)`` and ``import(`./pages/${name}.js`)`` reach the files they can name.
- `@use` of a Sass partial in a `.vue`, `.svelte` or `.astro` style block resolves.
- A jest `runner` that is already a jest package is read as written, and `eslint --format <name>` counts
  `eslint-formatter-<name>` as used.

## [0.1.0] - 2026-11-06

### Changed

- **Public API hardening** ([ADR 0046](adrs/0046-corrections-to-adr-0044-config-helpers-not-built-and-the-torn-read-limit-of-zero-trailers.md)):
  the `explain` result types, `PathKind` and the `skif-migrate` types are `#[non_exhaustive]` (`skif-migrate` gains
  `Input::new`, `RuleInfo::new` and `Options::new` with builders); the facade re-exports `PathKind` and the types of
  `UsageReport`; `skif-js`, `skif-js-plugins`, `skif-js-syntax` and `skif-edit` warn on missing documentation; the
  third-party type audit reads trait members and re-exports. The tests of `skif-migrate` and `skif-rules`, which read
  files of the repository, are not part of their published packages.
- **Faster resolution**: every resolver set of a run (scripts, per-package bundler extensions, styles) shares one
  `oxc_resolver` cache, so a path is looked up on disk once; a root's tracked paths are found in the git index by
  bisection instead of testing every tracked path.
- **`skif migrate` rule coverage and overrides**: which rules a tool runs comes from data tables checked against the
  rule list (`tool-rules.txt` beside the sherif and knip tables). The `exact` profile also turns off
  `catalog-not-used`, `catalog-missing-entry` and `unresolved-catalog-references` unless syncpack has a
  `policy: catalog` group, and `unparseable-files`, which knip does not report. knip's `treatConfigHintsAsErrors` is
  written as `stale-js-config = "warn"` because skif's hints differ from knip's; `--strict-hints` writes `error`. New
  `[policy] override-reading = "skif" | "syncpack"`: `syncpack` reads `resolutions`, `overrides` and `pnpm-overrides`
  as syncpack 15 does (the key is the dependency name; nested objects and `pnpm.overrides` of a manifest are not
  read), and migrating syncpack writes it
  ([ADR 0045](adrs/0045-migrate-tool-rule-tables-warn-for-config-hints-and-syncpack-override-reading.md)).
- **Distribution hardening**: `scripts/check-versions.sh` also compares the workspace packages of `Cargo.lock`, and
  `scripts/release-bump.sh` fails when `cargo update --workspace` fails. The glibc floor lives in
  `scripts/lib/libc.sh` (the Homebrew formula takes it from there) and `check-versions.sh` checks the build matrix
  against it. The Action and the pre-commit launcher pin `gh attestation verify` to the `publish.yaml` workflow
  (`--signer-workflow`). The Action `args` input accepts one argument per line. A 0.x release moves the floating tag
  `v0.M` instead of `v0` (`@v0.1` in the examples); `@v0.M` and `@vN` resolve to the newest matching release.
  `cargo binstall` installs the static musl archive on Linux, so it also works below glibc 2.34. Bun is tested on the
  newest 1.x, resolved from the registry. The repository guards and `npm-smoke.sh` have tests, and CI runs the facade
  unit tests and doctests as two invocations ([ADR 0043](adrs/0043-floating-tags-for-0x-releases-and-musl-binstall.md)).

### Added

- **`catalog-version-divergence`** ([ADR 0048](adrs/0048-catalog-version-divergence-and-reasons-on-ignores.md)): a warning
  on `catalog:` references to one dependency that resolve to different versions, which `version-mismatch` cannot see.
  `catalog-mismatch` names the catalog's version in its message.
- **Reasons on ignores** ([ADR 0048](adrs/0048-catalog-version-divergence-and-reasons-on-ignores.md)): `[[ignore]]` takes
  an optional `reason`, and `--require-ignore-reasons` fails a run on an entry of a file without one.
- **`[js]` configuration parity with knip** ([ADR 0034](adrs/0034-js-configuration-parity-with-knip.md)):
  `ignore-dependencies`, `ignore-binaries`, the new `ignore-unresolved` (top level and per workspace, with
  `extend-ignore-unresolved`) and `ignore-members` take regular expressions (`/^@org\/.+$/`, or knip's character
  test) next to globs, and an entry ending in `!` applies in production mode only. `entry` and `project` honour the
  production marker: `pattern!` is a normal pattern in a normal run, and a production run keeps only those.
  `[js] ignore-workspaces` skips workspaces in the JavaScript analysis only. `[js.workspaces."<dir>"]` gains `paths`
  (alias map), `plugins`, `ignore-unresolved`, `ignore-members`, `ignore-exports-used-in-file` and
  `include-entry-exports`, and `"."` is the root workspace. `ignore-exports-used-in-file` is `true` or a table of
  export kinds. Invalid globs, invalid regular expressions, empty `paths` entries and unknown plugin ids are
  configuration errors.
- **`stale-js-config` rule** (js, warning): reports `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved` and
  `ignore-workspaces` entries that matched nothing and `[js.workspaces]` keys that select no package, like knip's
  configuration hints; `[rules] stale-js-config = "error"` is `--treat-config-hints-as-errors` (`skif migrate` writes `warn` unless `--strict-hints` is given). There are 40 rules, 17
  of them `js` rules.
- **Override sections and custom types in the version policies**: `dependency-types` now also names `local`,
  `resolutions`, `overrides`, `pnpm-overrides` (`pnpm.overrides` and the `overrides` of `pnpm-workspace.yaml`) and the
  entries of the new `[[custom-types]]` (`name`, dotted `path`, `strategy` = `version`, `name@version` or
  `versions-by-name`). Their versions are instances like dependency entries, so version and semver groups pin, ban,
  align and range them (`engines.node`, `packageManager`, overrides), with fixes in `package.json` or
  `pnpm-workspace.yaml`. They are opt-in: the default scope is unchanged (ADR 0037).
- **Ignored semver groups**: `[[semver-groups]]` takes `ignored = true` instead of `range` (syncpack `isIgnored` and
  `isDisabled`); `SemverGroup::range` is now an `Option` and `SemverGroup::ignored()` builds such a group.
- **Built-in dependency families from sherif**: `turborepo`, `eslint-stylistic` and `lexical`, the full Storybook list
  and the Prisma adapters.
- **`[js] discovery`** (`auto`, `git`, `walk`, [ADR 0038](adrs/0038-git-index-file-discovery-and-resolver-probe-avoidance.md),
  corrected by [ADR 0042](adrs/0042-git-index-discovery-corrections-directory-times-lazy-existence-and-checksum.md)):
  when the analysed root is in a git work tree, source files are listed from `.git/index` (formats 2 to 4, read
  without running git) instead of walking the directories, and plugin config searches use the same listing. `auto`
  (the default) `stat`s each tracked directory and lists from disk any directory modified at or after the index time,
  so untracked and deleted files are found as the walk finds them; `git` trusts the index alone (faster, blind to files
  created or deleted since the last `git add`); `walk` always walks. The index checksum is verified, SHA-256
  repositories are refused, a tracked symbolic link is listed as a file or directory by its target, a conflicted file
  is listed once, and the parsed index is shared by every root of a run. `skif_js::discovery_file_system` returns the
  file system the analysis lists through.
- **`FileSystem::kind`** and `PathKind` in `skif-core`: file, directory or neither in one lookup. `CachedFs` keeps the
  answer, so a name probed as a file and as a directory costs one `stat`, and lists a directory once three of its
  children were probed.
- **`disk_tree` bench** in `skif-js`: a generated on-disk monorepo (more than 20 000 directories, 21 000 modules, pnpm
  `node_modules` links, a git index) for the walk, the index and name probing.
- **`skif migrate`**: translates the configuration of sherif (the `sherif` key of `package.json`), syncpack
  (`.syncpackrc*`, `syncpack.config.*`, the `syncpack` key) and knip (`knip.json(c)`, `knip.{js,ts,...}`, the `knip`
  key) into a `skif.toml`, reading JavaScript and TypeScript configs statically and the flags of the `package.json`
  scripts that run each tool. `--from sherif|syncpack|knip|auto`, `--profile exact|full`, `--merge`, `--force`,
  `--dry-run`, `--output`, `--source`, `--strict` and `--format json`; the report lists what was translated, what skif
  covers by default, what needs a human and the checks skif runs that the old tools did not. New crate `skif-migrate`;
  `skif-js-plugins` makes `load_config` and `scripts::split_commands` public. See ADR 0039 and the migration guide.
  `--merge` edits the existing file in place (comments and order stay, new tables go at the end under a banner), treats
  the `off` values of the `exact` profile as replaceable (`# skif migrate: not checked by <tools>`, plus a
  `# skif migrate: sources = [...]` line), so `--from sherif` then `--from knip --merge` ends with the same checks as
  `--from sherif,knip`; `--force` writes the existing `.skif.toml`. syncpack groups translate `**`, `hosted-git`,
  `resolutions`, `overrides`, `pnpmOverrides` and the default scope of syncpack 15; a custom type named like a built-in
  token is renamed; a `policy: catalog` group with selectors is left to a human. A rule that sherif ignores but syncpack
  still checks stays on. Per-script flags that disagree are combined with a note, per-workspace knip settings stay in
  their workspace, knip's issue types follow knip 6 (`--dependencies` covers the catalog types, production drops the dev
  and catalog types, `classMembers` is not a type), and scripts behind `npm run -- `, `CI=1`, `cross-env`, `dotenv --`,
  `turbo run`, `concurrently` and `sh -c` are read. See ADR 0041.
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
- **Karma function-form configs** (`module.exports = function (config) { config.set({ … }) }`) are read by static
  interpretation; no project code runs.
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
  repository ([ADR 0030](adrs/0030-harness-known-deviations-and-comparison-scope.md)). The namespace rules are
  compared with the `exports` and `types` items knip reports with a `namespace`, and with `nsExports` and `nsTypes`
  ([ADR 0033](adrs/0033-knip-namespace-findings-in-the-parity-harness.md)).
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
- **JS analysis on oxc** with 17 rules: `unused-files`, `unused-dependencies`, `unused-dev-dependencies`,
  `unused-optional-peer-dependencies`, `unlisted-dependencies`, `unlisted-binaries`, `unresolved-imports`,
  `unused-exports`, `unused-types`, `unused-namespace-exports`, `unused-namespace-types`, `unused-enum-members`,
  `unused-namespace-members`, `duplicate-exports`, `circular-imports`, `unparseable-files` and `stale-js-config`. Unused files, exports, types and dev dependencies default to warnings. Production mode,
  `--no-js` and `[js]` settings per workspace are supported.
- **A plugin table of 102 plugins** that reads the static configuration of common tools to find entry files and the
  dependencies they use, without running any tool: `typescript`, `eslint`, `prettier`, `vite`, `vitest`, `jest`, `next`, `storybook`, `playwright`, `tailwind`,
  `postcss`, `babel`, `webpack`, `rspack`, `rollup`, `tsup`, `turbo`, `husky`, `lint-staged`, `commitlint`,
  `changesets`, `github-actions`, `mastra`, `aws-cdk`, `browser-extension`, `tsdown`, `oxlint`, `angular`, `karma`,
  `esbuild`, `svelte`, `nuxt`, `astro`, `drizzle`, `ts-node`, `tsx`, `nx`, `cypress`, `mocha`, `sveltekit`, `prisma`,
  `graphql-codegen`, `docusaurus`, `vitepress`, `wrangler`, `expo`, `react-native`, `metro`, `release-it`,
  `semantic-release`, `lefthook`, `simple-git-hooks`, `stylelint`, `markdownlint`, `cspell`, `nodemon`, `pm2`, `pnpm`,
  `yarn`, `syncpack`, `node`, `expo-router`, `tanstack-router`, `ava`, `biome`, `c8`, `dotenv`, `electron-vite`,
  `gatsby`, `i18next-parser`, `jasmine`, `knex`, `lingui`, `msw`, `nest`, `netlify`, `nitro`, `nyc`, `oclif`,
  `panda-css`, `plop`, `preconstruct`, `react-router`, `remark`, `remix`, `rolldown`, `rsbuild`, `rslib`, `sanity`,
  `sentry`, `size-limit`, `stryker`, `svgo`, `svgr`, `swc`, `tsd`, `typedoc`, `unbuild`, `unocss`, `vercel`, `vike` and
  `webdriver-io`.
  The set is extended in [ADR 0025](adrs/0025-plugin-set-extension-thirty-five-more-tools.md), ADR 0029 and
  [ADR 0036](adrs/0036-thirty-nine-more-knip-plugins-by-real-world-usage.md).
- **Output formats**: human, JSON, SARIF and GitHub annotations, selected with `--format`.
- **Atomic fixes**: `--fix` and `--fix-unsafe` plan all edits first, write them atomically and lint again;
  edits keep key order, indentation and newline style. `--fix` runs the package manager's install unless
  `--no-install` is given.
- **`skif.toml` configuration** with a committed JSON Schema (`skif schema`), `skif init` and `skif rules`.
- **npm package as a napi-rs addon** for non-EoL Node.js (22, 24, 26) and Bun 1.x on macOS and
  Linux (glibc and musl), plus release
  archives for Windows, `cargo binstall`, Homebrew and Scoop.
- **Git hooks** for pre-commit, Poly and Lefthook, and a GitHub Action (`Goldziher/skif@v0.1`).
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

- **JS analysis follows knip more closely**: `mergeConfig`, `defu`, `deepmerge`, `merge` and `Object.assign` in tool
  configs are evaluated as merges (an unknown operand leaves an unknown spread); a config no plugin could read sets
  `entries_incomplete` for every plugin that reads configs, not only five; package `imports` that point at build output
  map back to sources; `.mjs`/`.cjs`/`.js` output maps to every source extension; tsconfig `rootDirs` are followed for
  relative imports; regular-expression `find` entries of bundler aliases and Jest `moduleNameMapper` prefixes become
  aliases; `xargs`, `nice`, `sudo`, `timeout`, `watch` and `unbuffer` are unwrapped in scripts, and a binary run with
  `yarn workspace <name>`, `pnpm --filter <name>` or `npm -w <name>` is credited to that workspace.
- **Test files are entries only through a test-runner plugin** or `[js] entry`, no longer in every project.
- **Unresolvable specifiers that start like a package name are dependencies**, not unresolved imports; missing relative
  files that Git ignores or that are JSON or foreign file types are not reported from scripts.
- **CommonJS named exports** (`exports.x =`, `module.exports.x =`, `module.exports = { a, b }`) are tracked and reported
  as unused exports.
- **JS analysis performance**: the resolver finds the scopes of an importer by ancestor lookup and skips the
  lookup-directory probe when no package has lookup directories; plugin setup and the installed-manifest reads run per
  package in parallel; entry collection indexes candidates by owner and binary-searches the files below a package;
  `tsconfig` files are parsed once per run in the dependency pass; plugin config globs are compiled once;
  `declares_module` is a set lookup. A tracked file the git index lists but that is missing on disk is an empty module
  instead of an unparseable file.
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
  documentation and checked by a test ([ADR 0032](adrs/0032-third-party-types-in-the-facade-api.md)).
- **A `Linter` shared by concurrent runs keeps one cache for them**: the cache of the built-in JS analysis is emptied
  when the first of a group of overlapping runs starts and when the last one ends, so `skif -r` no longer wipes the
  listings that the roots already running just read, and an idle `Linter` holds nothing.
- **`skif config -r` walks each tree once** (`unapplied_config_files_all`) instead of once per enclosing root.
- **Internal errors**: a duplicate rule id is reported with kind `internal`, not `config`.
- **glibc floor of the Linux archives is real**: the `*-linux-gnu` archives are linked against glibc 2.34
  (cargo-zigbuild) instead of the runner's glibc 2.39, and the build fails if a binary imports a newer symbol. The
  GitHub Action, the pre-commit launcher and the Homebrew formula install the musl archive when the glibc is older than
  2.34 or cannot be identified; `cargo binstall` hosts below 2.34 pass `--targets <arch>-unknown-linux-musl`. The
  release pipeline runs both flavours in Debian 11, AlmaLinux 8 and 9 and Ubuntu 22.04 containers before a release.
- **GitHub Action supply chain**: a release of the action installs the skif version it was published with (a SHA or
  floating-tag pin no longer floats), a cache hit is re-checked against the release's `checksums.txt`, and archives are
  checked with `gh attestation verify` against the build-provenance attestation the release workflow now creates
  (`verify-attestation: false` for mirrors; a `gh` without the command only warns). The pre-commit launcher checks the
  attestation too when an authenticated `gh` exists and says so when it cannot.
- **Release pipeline order**: the GitHub release is made public only after crates.io is published and the npm package
  runs; `cargo binstall` is verified after that. The dry run also starts for changes to the release inputs and weekly,
  and the Windows arm64 archive is unpacked with `Expand-Archive`.
- **CI covers the supported versions of ADR 0027**: the npm addon and published-package checks run on Node 22, 24 and
  26 and the latest Bun 1.x; the end-to-end suite runs the latest two majors of npm, pnpm and Yarn 2+ (resolved from
  the registry) and the latest Bun.
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
- The JSON Schema of `skif.toml` offers `level` (with `severity` as its alias) in a rule table, and the
  configuration page documents the rule options (`mode` of `member-toolchain-fields`, `ignore-tags` of
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

- **Release workflow**: moving the floating tag no longer fails on an unset variable after the tag was pushed; the
  logic is `scripts/release-floating-tag.sh`, tested in `scripts/test-release-scripts.sh`, and the tag trigger only
  matches full `vX.Y.Z` versions.
- **`skif migrate`**: a trailing `# comment` after a `pnpm-workspace.yaml` package pattern is not part of the pattern.
- **Monorepo JS corrections** ([ADR 0044](adrs/0044-monorepo-js-corrections-builtins-html-entries-referenced-paths-extglob-helpers-and-stylesheets.md)):
  `buffer/` and `events/` name npm packages, not builtins; the vitest plugin adds `index.html` scripts without the vite
  plugin; the `paths` of a `tsconfig` referenced in another package stay with that package when the referencing package lists it as a
  dependency (an unlisted one still resolves through the referenced paths); extglob groups
  (`*.(ts|tsx)`, `@(a|b)`, `+(…)`) work in plugin globs; stylesheet imports through `baseUrl`, `paths` and `resolve.modules` use the
  stylesheet resolver, partials included; bundler `resolve.extensions` apply per package.
- **Catalog rules respect version groups** ([ADR 0040](adrs/0040-version-groups-own-their-instances-the-catalog-rules-leave-claimed-instances.md)):
  an instance a configured `[[version-groups]]` entry matches belongs to that group, whatever its policy
  (`pinned`, `banned`, `ignored`, `snapped-to`, an aligned policy with a selector), as in syncpack's first-match-wins
  order. `catalog-not-used`, `catalog-mismatch`, `catalog-missing-entry` and `unresolved-catalog-references` skip it,
  so no instance gets two reports or two conflicting fixes, and a pinned "baseline" package stays off the catalog
  while the others use `catalog:`. Claimed instances do not count toward `min-packages` or the version of a created
  entry. `catalog-mismatch` is a warning by default (it only reports with `mode = "off"`).
- **Layered custom types**: a `dependency-types` list may name a `[[custom-types]]` entry defined in an inherited
  layer; references are checked on the merged configuration, and an error found there names the file and line that set
  the key.
- **Override selectors**: the dependency of `react@>=18`, `parent@1>child@2`, `**/name`, `a/**/@scope/name@^1` and
  `foo@npm:@scope/bar@1` is parsed by one function shared with `unused-catalog-entries`, which now sees
  `**/name` references; `unresolved-catalog-references` also checks override sections. Override values that are
  references (`$name`, `-`) are not versions, and the `overrides` of `pnpm-workspace.yaml` and its `packages` are read
  only when pnpm is the package manager.
- **`pnpm-workspace.yaml` keys** with a `:` inside (`foo@npm:bar`) or whitespace before the colon get a location.
- **Catalogs**: a whole-number YAML version stays a version, a fractional one (`1.10`, read as `1.1` by YAML) is no
  longer resolved to the wrong text; new entries go to the file that declares their catalog and to `catalogs.default`
  when the default catalog is declared there; `catalog: react` (a space) resolves nothing.
- **`CachedFs`** answers a probe in another letter case like the disk does, whatever the number of probes before it,
  and `OsFs` keeps a directory entry whose type the record lacks.
- **`[overrides.js]` replace keys** can clear an inherited list with `= []`; a mistyped export kind in
  `ignore-exports-used-in-file` is reported by name; an ignore file of comments builds no matcher.
- A group with no `dependency-types` covers the types `[policy] dependency-types` governs (the docs said every type).
- **Production mode keeps unmarked entries out of the project**: with `production = true`, an `entry` pattern that does not
  end in `!` (and the project files that tools add) is removed from the project, so knip's non-production files are
  no longer reported by `unused-files`.
- **`*` stops at `/` in `ignore-workspaces` and in `[js.workspaces]` keys** (`packages/*` no longer selects
  `packages/a/nested`), as in knip; the keys are compiled once per run.
- **`stale-js-config` does not judge `ignore-binaries` when `unlisted-binaries` is off.**
- **Config evaluation**: `merge` and `mergeWith` count as config combinators only when they are not imported from
  another package (an `RxJS` `merge` is no longer evaluated as an object merge).
- **Plugins follow knip more closely**: `jasmine` and `ava` keep negated patterns (`!spec/skip.js`) as negated entries,
  and `jasmine <dir>`, `stryker run <config>`, `tsd --files <glob>` and `tsd <dir>` and `typedoc --plugin/--theme` are
  read from the scripts; `ava` reads `nodeArguments` like a `node` command; `knex` credits the driver of
  `knex({ client })` calls in source files; `react-router` reads `flatRoutes()` and route file names with `$`, `[` and
  `+`; `rsbuild` (and `swc-loader` rules of `webpack` and `rspack`) credit `swc` plugins; `gatsby-node` babel plugins,
  `size-limit.config.*`, the electron-vite default entries next to configured ones, `load-plugin` naming in `remark`,
  Biome's `"//"` root config, and `c8 check-coverage` are handled. The remaining differences are listed in the plugin
  reference.
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
