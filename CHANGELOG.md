# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- Keep a Changelog repeats Added/Changed/Fixed headings per version. -->
<!-- markdownlint-disable MD024 -->

## [Unreleased]

- GitHub Action: with `from-sherif: true`, `baseline write`, `prune` and `check` read the translated sherif settings through a temporary configuration that extends the translation and then the baseline file.

### Added

- `toolchain-pin-drift` (`warn`): Node and package-manager versions pinned in `.nvmrc`, `.node-version`,
  `.tool-versions`, `mise.toml`, Dockerfiles (`FROM node:<version>`, `ARG NODE_VERSION`, `corepack prepare`,
  `npm install -g pnpm@<version>`) and GitHub workflows (`actions/setup-node`, `pnpm/action-setup`) are checked
  against the root: the first root version file, else `engines.node`, and `packageManager`. A finding names the file,
  the line and both versions; a range that admits the root's version is no drift. A Dockerfile that runs
  `corepack enable` without `corepack prepare` is reported at `info`. Options `kinds`, `files` and `ignore-files`, and
  `# skif-ignore-next-line` / `# skif-ignore-file` comments with a required reason
  ([ADR 0087](adrs/0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md),
  [#138](https://github.com/Goldziher/skif/issues/138)).
- `foreign-package-manager-command` (`warn`): Dockerfile `RUN` instructions, workflow `run:` steps and `*.sh` scripts
  (plus the `files` globs) that run a package manager other than the workspace's are reported: `npm ci`, `npm install`,
  `npm run`, `npx` and the like in a pnpm or yarn workspace, a bare `yarn` or `yarn <script>` in another. Read-only
  commands (`npm view`, `npm whoami`), a global install of the workspace's own manager and files of projects outside
  the workspace are not; the same options and suppression comments as `toolchain-pin-drift` apply
  ([ADR 0088](adrs/0088-foreign-package-manager-commands.md), [#139](https://github.com/Goldziher/skif/issues/139)).
- `invalid-workspace-file` (`error`): a member `package.json` that is not valid JSON or cannot be read, a workspace
  pattern that cannot be used and a `.yarnrc.yml` that is not valid YAML are reported, at the syntax error or the
  pattern. Discovery left them out of the run without a word, so a workspace with a broken member passed
  ([ADR 0095](adrs/0095-untracked-files-member-manifest-errors-and-converging-fixes.md)).
- `incomplete-packages[]` entries of the JSON report carry a `scope`: `"package"` when no file of the package was
  judged unused, or `{"within": [globs]}` with the globs, relative to its `dir`, of the only files left unjudged
  ([ADR 0065](adrs/0065-narrowed-scope-for-incomplete-entries.md)); the human note names the globs after the package.
  `skif_core::IncompletePackage` gains the `scope` field and `with_scope`, and `IncompleteScope` moved from
  `skif-js` to `skif-core` (`skif_js::IncompleteScope` re-exports it).
- `skif_rules::toolchain_provider()` provides the rules over toolchain files, and `skif_rules::providers()` includes
  it. The rules read those files through `LintContext::fs`, the file system the run is given
  (`skif_lint::RunOptions::with_fs`), as the rules over a lockfile and patch files do
  ([ADR 0095](adrs/0095-untracked-files-member-manifest-errors-and-converging-fixes.md)).

### Changed

- `[js] discovery = "auto"`, the default, walks the directories instead of listing tracked directories from the git
  index. The index cannot tell whether an unchanged directory holds an untracked file created before the index was
  last written (a `git add` or `git commit` of another file does that), and such a file, an entry or the only importer
  of another file, then went missing: false `unused-exports` and `unused-files`, and a `--fix --allow-remove-files`
  that deleted a tracked file only it used. `discovery = "git"` now does what `auto` did (the index, with every
  directory modified since it was written listed from disk) instead of trusting the index alone, and its
  `unused-files` findings offer no fix that deletes the file
  ([ADR 0095](adrs/0095-untracked-files-member-manifest-errors-and-converging-fixes.md)).
- The shell tokenizer of the `package.json` script parser moved from `skif-js-plugins` to `skif_core::shell`
  (`tokenize`, `Tokens`, `Segment`, `Word`), so rules outside the JS analysis split command lines with it.
- **Breaking** for library users: `skif_config::Assignment` and `skif_core::IncompleteEntries` (both re-exported by
  `skif`) are `#[non_exhaustive]`, so a later field is not a breaking change. Build them with `Assignment::new` and
  `IncompleteEntries::new(cause).with_plugin(..).with_file(..)`.

### Changed

- `RuleMeta` gains `version_scoped`, set with `RuleMeta::version_scoped()`; `IgnoreEntry` gains `files` and
  `with_files`; `OverrideScope` gains `unanchored()`.
- syncpack format keys (`sortAz`, `sortFirst`, `indent`, ...) turn `package-json-format` and `unsorted-dependencies`
  on only when a script runs `syncpack format`; otherwise the settings are written with the rules off and a note.
- syncpack's `policy: catalog` migrates to `[catalog] mode = "enforce"` with `min-packages = 1`.
- `unsynced-dependency-family` honours `[policy] dependency-types`, so the sherif migration (`prod`, `dev`) leaves
  optional dependencies out as sherif does.
- Template expressions of `.vue`, `.svelte`, `.astro` and `.mdx` files are parsed as JavaScript under the scopes of
  their block constructs (Vue `v-for`, `v-slot`, `#name`, `slot-scope`; Svelte `{#each}`, `{#await}`, `{:then}`,
  `{:catch}`, `{#snippet}`, `{@const}`, `let:`; arrow parameters such as `items.map((item) => …)`), so a member that
  a template local of the same name as a namespace import reads is no use of that namespace's export (a named import
  keeps its export used, as in knip)
  ([ADR 0084](adrs/0084-template-scopes-auto-imported-vue-components-and-marko-files.md),
  [#102](https://github.com/Goldziher/skif/issues/102)).
- A component that `unplugin-vue-components` registers in `components.d.ts` from a file of the project is used only
  where a template names it as Vue resolves the tag (`AppButton`, `app-button`, `appButton`, `LazyAppButton`), in a
  static `is` attribute or in a `resolveComponent('AppButton')` call, as knip compiles it: one no template uses is
  reported by `unused-files`. Native elements (`<button>`) name no component, the imports added for components are
  no `circular-imports` cycle, and a package that may use its components in files without tags (a `.tsx` or `.jsx`
  file, a VitePress or VuePress site, or an `include` of the plugin beyond `.vue` files) credits all of them as before
  ([ADR 0084](adrs/0084-template-scopes-auto-imported-vue-components-and-marko-files.md)).
- `ConfigValue::Regex` of `skif-js-syntax` holds the `flags` of the literal besides its `pattern`. `PluginInput` of
  `skif-js-plugins` gains `EntryRegex`, `PluginOutput` gains `entry_regexes` and `WorkspaceInputs` of `skif-js` gains
  `entry_regex`, for Jest's `testRegex` ([#100](https://github.com/Goldziher/skif/issues/100)).
- `BinaryUse` and `DepRef` of `skif_js_plugins::scripts` gain `dir`, the directory a command runs in after `cd`,
  `pushd` or a package manager's directory flag; `skif_js_plugins::scripts` also gains `is_powershell_command` and
  `without_commands`. `IncompleteCause` of `skif-core` gains `NoEntries`.
- `PluginOutput` of `skif-js-plugins` and `WorkspaceInputs` of `skif-js` are `#[non_exhaustive]`: an input source
  starts from `WorkspaceInputs::default()` and sets the fields it knows.
- `Report::other_project_files` lists every file with an absolute path (the files below the workspace root were
  relative to it) and also lists the files outside the root that the analysed code reaches;
  `Report::ancestor_dependency_uses` also lists the dependencies those files import, with the directory of the manifest
  that lists them ([ADR 0085](adrs/0085-files-other-roots-reach-count-as-used.md),
  [#100](https://github.com/Goldziher/skif/issues/100)).
- The knip comparison sets apart findings that follow from where each tool looks, decided from paths and manifests:
  unused dependencies and script binaries skif reports on a `package.json` that is not one of knip's workspaces
  (`knip-unjudged-manifest`), unused files and listed unlisted dependencies knip reports below a project that is not
  one of its workspaces (`non-member-project`), knip's findings below directories skif's file discovery skips
  (`skipped-directory`), and TypeScript sources whose built path the package publishes (`published-source`).
  `fixture-file` covers every issue type skif does not report in a fixture directory, not only unused files
  ([ADR 0080](adrs/0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md),
  [#27](https://github.com/Goldziher/skif/issues/27)).
- The knip comparison runs skif recursively on babel, whose `scripts/repo-utils` and
  `scripts/integration-tests/fixtures/nextjs-10` are projects of their own outside the workspace patterns, whose files
  knip reads from the root.
- Every speed and knip agreement table in the README and the comparison guide is generated, and names the skif commit,
  tool versions and machine it comes from: `mise run bench:publish` writes the speed tables, `mise run parity:publish`
  the agreement tables, and both refuse to write when a tool was not timed or the baseline is stale. The benchmark
  times the sherif, syncpack and knip versions pinned in `tools/bench/corpus.toml` instead of whatever is on `PATH`,
  runs skif with `--recursive` where the corpus marks a repository recursive, and measures 15 runs. The knip
  comparison sets apart findings inside a file the other tool reports unused (`in-knip-unused-file`,
  `in-skif-unused-file`) and applies `knip-config-error` to both sides, reads each repository's install state from its
  checkout, marks n8n as not a valid comparison, runs knip on rushstack in a scratch copy with a root manifest made
  from `rush.json`, and refuses to check against a baseline recorded with another knip version or category set
  ([#27](https://github.com/Goldziher/skif/issues/27), [#95](https://github.com/Goldziher/skif/issues/95)).

### Added

- `[[ignore]] files`: globs of the files a finding is located in, relative to the workspace root. `skif baseline`
  records a finding about a file (`unused-files`, `unused-exports`, `circular-imports`) with its file, so a new unused
  file or export elsewhere in the package fails the run
  ([ADR 0093](adrs/0093-baseline-entries-follow-what-a-finding-is-about.md)).
- `skif baseline write` and `prune` take the lint flags that change the findings (`--config`, `--rule`, the ignore
  flags, `--no-js`, `--production`, `--strict`, `-W`, `--include`, `--exclude`, ...), and every `skif baseline` command
  covers several roots as a lint run does (`-r`, or a `PATH` without a `package.json` above it), each root with its own
  baseline file ([ADR 0093](adrs/0093-baseline-entries-follow-what-a-finding-is-about.md)).
- A fix run of a single root warns when it would remove a dependency and other roots lie below it, whose uses only
  `skif -r` credits.
- A lint run warns about an `[[overrides]]` pattern that leaves the directory it is anchored at (`../packages/*`), which
  matches nothing.
- `skif migrate --force` warns when the file it replaces had an `extends` (a baseline file, a shared preset).
- `banned-dependency` judges what `pnpm-lock.yaml` resolves for a `[[version-groups]]` entry with
  `scope = "lockfile"` (only with `policy = "banned"`): every resolved `name@version` the group bans, transitive ones
  included, is reported once at its lockfile entry with the shortest chain an importer pulls it in through, so
  removing the `overrides` entry that kept a package out fails the lint. The group claims no manifest entry
  ([ADR 0090](adrs/0090-lockfile-scopes-for-banned-groups-and-dependency-families.md),
  [#141](https://github.com/Goldziher/skif/issues/141)).
- `[[dependency-families]]` entries take `scope = "importer"` or `scope = "lockfile"`: `unsynced-dependency-family`
  then requires the members to resolve to one version per importer or across `pnpm-lock.yaml`, and with
  `anchor = "<package>"` each member to one version per major of the anchor (`@types/react` against `react`)
  ([ADR 0090](adrs/0090-lockfile-scopes-for-banned-groups-and-dependency-families.md),
  [#141](https://github.com/Goldziher/skif/issues/141)).
- New rule `stale-patched-dependencies` (workspace, warning): a `patchedDependencies` key of the root
  `pnpm-workspace.yaml` or `package.json` whose `name@version` `pnpm-lock.yaml` no longer resolves or resolves without
  a patch hash, a patch file that does not exist or lies outside the `allowed-patch-paths` globs, and a `*.patch` file
  no entry references in the patch directories (`patch-dirs`, by default those of the referenced patches and
  `patches`) ([ADR 0091](adrs/0091-stale-patched-dependencies.md), [#142](https://github.com/Goldziher/skif/issues/142)).
- The lockfile checks read `pnpm-lock.yaml` in lockfile format 9 (pnpm 9 and 10) only, on the first run that needs it.
  Without one, with another manager's lockfile, or when it does not parse, the rule reports that nothing was checked.
  `skif-workspace` exports the model (`PnpmLockfile`, `LockedDependency`, `Snapshot`, `LockedPatch`, `Resolutions`)
  and `WorkspaceError::InvalidLockfile`; `YamlSource::entry_span` locates an entry of any top-level mapping.
- `RunOptions::with_fs` of `skif-lint` passes the host's file system to rules that read files beyond the discovered
  workspace, through `LintContext::fs`; it defaults to the real file system, and `Linter` passes the one it was built
  with. `VersionGroup::scope` (`GroupScope`) and `DependencyFamily::scope` (`FamilyScope`) and `anchor` are new
  configuration fields.
- `stray-lockfile` reports a lockfile (`package-lock.json`, `npm-shrinkwrap.json`, `yarn.lock`, `pnpm-lock.yaml`,
  `bun.lock`, `bun.lockb`) in a workspace member, and a lockfile at the root that another package manager than the
  workspace's writes; its unsafe fix deletes the file. `nested-workspace-root` reports a `pnpm-workspace.yaml` in a
  member, also one a recursive run lints as a root of its own. Both search the root and the directories the
  workspace patterns select, leave out files git ignores, and take `allow`, a list of paths kept on purpose, each a
  string or `{ name, reason }`; an entry that names nothing is reported, and `--require-ignore-reasons` covers the
  entries. `DiscoveryFacts::package_manager_files` of `skif-workspace` lists the files they judge
  ([ADR 0089](adrs/0089-stray-lockfiles-and-nested-workspace-roots.md),
  [#140](https://github.com/Goldziher/skif/issues/140)).
- `unlisted-dependencies` reports a package that a tool config names itself and no manifest lists, at the config
  with the detail `ConfigReference`: a `tsconfig` `extends`, `types` or plugin, an Oxlint `jsPlugins` entry, an
  ESLint plugin, parser or shareable config, a Babel preset or plugin, a PostCSS plugin, a Prisma generator and the
  other packages plugins read from configs. The config belongs to the innermost package whose directory holds it.
  Packages that a command in a config names, that a plugin assumes without a config, and optional references are not
  reported; `ignore-dependencies` and `ignore` apply, a gitignored config or one in a fixture directory (unless
  `include-fixtures`) is not judged, and production mode judges no config. A `tsconfig` compiler option of a shared
  base config is satisfied by the package extending it as well as the base's owner, and a `types` entry by the
  package or its `@types` package. Plugins record the dependencies a non-config file names with
  `PluginInput::FileDependency` and compiler options with `PluginInput::CompilerDependency`; `DepRef` has
  `named_by_config`, `type_only` and `reader_may_list`, with `DepRef::new` and `DepRef::merge`
  ([ADR 0086](adrs/0086-packages-tool-configs-name-are-unlisted-at-the-config.md), [#96](https://github.com/Goldziher/skif/issues/96)).
- `.marko` files are read: their `import` statements and `import()` calls, and the imports of their `<style>` and
  concise `style { … }` blocks. A reached `.marko` file uses `marko`
  ([ADR 0084](adrs/0084-template-scopes-auto-imported-vue-components-and-marko-files.md)).
- `skif-js-syntax` exports `registered_components`, which reads the components a generated `components.d.ts`
  registers, `ModuleInfo::template_tags`, the tag names of a `.vue` template with its static `is` values and the
  names of `resolveComponent` calls, and Vue's tag resolution (`camelize`, `capitalize`, `resolved_names`,
  `registered_name`, `is_native_tag`).
- Eight plugins read more of their configs, still without running them
  ([ADR 0078](adrs/0078-plugin-gaps-config-functions-file-tasks-and-templates.md),
  [#103](https://github.com/Goldziher/skif/issues/103)):
  - `sst` reads the handler shorthand of SST 2 constructs in the stacks `sst.config.ts` imports (`routes`,
    `subscribers`, `notifications`, `targets`, and `function`, `job` and `consumer` strings).
  - `serverless-framework` expands `${self:…}` variables and quoted fallbacks in a `handler`, reads any other
    variable as a `**` or `*` wildcard and takes a glob handler as an entry glob.
  - `vue` calls a function `configureWebpack` statically with a base config and reads its result.
  - `eleventy` reads `addPassthroughCopy` calls.
  - `rstest` applies `exclude` and negated `include` patterns as `!` entries, which the default test globs do not
    override. `rs.mock()` and the other `rs` and `rstest` module calls are mock imports.
  - `mise` reads file tasks (`mise-tasks/`, `.mise/tasks/`, `.config/mise/tasks/`, ..., or `task_config.includes`,
    which replaces them).
  - `taskfile` expands `{{.NAME}}` templates from `vars`, `ROOT_DIR` and `TASKFILE_DIR`, and reads the rest of a
    command around any other template.
  - `react-email` credits only the preview server package of the installed (else declared) major version.
- `skif-js-syntax` exports `extract_property_calls`, which calls a function property of a config statically, and
  `extract_method_calls`, which evaluates the arguments of method calls.
- `skif-js-syntax`: `ImportedName` gains `nested_members`, the members read through a member of the binding
  (`ns.E.A` reads `A` of `E`), for each member used only that way. A struct literal of `ImportedName` needs the new
  field.

### Fixed

- An installed package whose manifest declares its binaries with `directories.bin` instead of `bin` provides every
  file below that directory as a binary, named by its file name as npm links it; its binaries were reported as
  unlisted and the package as unused.
- An ESLint config that extends a built-in config (`eslint:recommended`, `eslint:all`) credits `eslint`, as knip does,
  so an `eslint` listed only at the root and enabled by a member's config is no longer reported unused.
- `skif migrate` from knip: with a `workspaces["."]` table, the top-level `entry`, `project` and `ignore` are not
  written (knip reads the root workspace from that table alone), instead of being merged with the table's lists into
  `[js]`, which widened the entries.
- npm package: a panic in `skif` exits `2` (could not run) with the JSON `error` document (kind `internal`) under
  `--format json`, as the native binary does; it surfaced as a JavaScript exception with exit code 1 and no document.
  `skif_cli::run` catches the panic.
- An unknown rule ID in the configuration names the file and line that set it (`preset.toml:4:1: unknown rule ...`,
  with `path`, `line` and `column` in the JSON error), also in an extended preset; for a package file the error names
  the file. It had no location at all. `skif::Target::locate` attributes the error for library hosts.
- `[js.workspaces."<dir>"]` tables accept `extend-ignore`, `extend-ignore-dependencies`, `extend-ignore-binaries` and
  `extend-ignore-unresolved`, which append to the list the table inherits through `extends`, as
  [ADR 0074](adrs/0074-reasons-on-js-ignore-entries.md) says; they were rejected as unknown fields.
  `skif_config::JsWorkspace` gains the four fields.
- A file that starts with a UTF-8 byte order mark no longer reports every first-line column one too far (human,
  compact, JSON, SARIF and LSP), since editors and code hosts hide the mark; `LineIndex` counts no column for it.
- `skif baseline` records `name@specifier` only for the rules whose finding is about the specifier (`RuleMeta::version_scoped`:
  `version-mismatch`, `semver-range-mismatch`, `policy-conflict`, `unsynced-dependency-family`,
  `incompatible-peer-dependencies`, `catalog-mismatch`, `catalog-version-divergence`); any other finding records the
  dependency name, so bumping a baselined `@types/node` no longer reopens its `types-in-dependencies` finding
  ([ADR 0093](adrs/0093-baseline-entries-follow-what-a-finding-is-about.md)).
- `skif baseline write` and `prune` exit `2` when the configuration does not apply the baseline: no configuration file
  of the run extends it, or a `reset = ["ignore"]` applied after it drops its entries; `write` reported success while
  the lint run still failed ([ADR 0093](adrs/0093-baseline-entries-follow-what-a-finding-is-about.md)).
- `skif baseline write` and `prune` set aside only the entries of the baseline file while they lint; a hand-written
  `[[ignore]]` of another configuration file equal to a baseline entry was set aside too, so `write` recorded its
  finding into the baseline and the suppression moved there.
- `skif baseline check --file` in a directory that does not exist names the directory instead of calling the revision
  bad.
- The `--format human` help of `skif baseline`, `config`, `migrate`, `rules` and `update` no longer calls every output
  an aligned table.
- The exact sherif migration leaves out of `version-mismatch` the specifiers sherif never compares (`*`, `latest`, tags,
  `||` ranges, git, url, file, link, `workspace:` and `npm:` aliases), through a last `ignored` version group.
- The script parser of `skif migrate` reads short flags written `-x=value` and `-xvalue`, and takes sherif's values
  whole, as sherif does, instead of splitting them at commas.
- A syncpack custom type with `source: "PackageJson"` is migrated instead of being dropped as reading another file.
- `unsynced-dependency-family` no longer compares a dependency listed in two sections of a package with itself.
- The GitHub Action appends `--fail-on-warnings` and the migrated `--config` of `from-sherif` to lint runs only, and
  knows `baseline` as a subcommand, so `args: baseline check --base origin/main` no longer fails on an unknown flag.
- A release fails before anything is published when an npm package (the umbrella or a platform package, Windows ones
  included) does not exist yet or `CHANGELOG.md` has no section for the version; the crates are published only after
  the Windows addon checks and an install smoke of the packed tarballs with npm and Bun on Windows.
- cargo-semver-checks runs over every published crate and writes its result to the job summary.
- Libc detection no longer takes a glibc host with an installed musl loader for musl.
- The `version-mismatch` message for specifiers that are not all semver versions no longer says none of them is
  semver: it reads "differs from the other specifiers, and not every one of them is a semver version to align on",
  which also fits a semver specifier among them.
- One `--fix` or `--fix-unsafe` run applies a value fix (`version-mismatch`, `semver-range-mismatch`,
  `catalog-not-used`) in an object that `unsorted-dependencies` also sorts. The two were planned as overlapping edits,
  so the value fix was skipped as a conflict and only a second run applied it; such fixes are now planned together
  ([ADR 0095](adrs/0095-untracked-files-member-manifest-errors-and-converging-fixes.md)).
- A bare-major range such as `^2` or `~1` stands for the versions it covers: `"a": "^2"` on a workspace package at
  `2.3.0` is valid, and `^2` overlaps `~2.1.0` in a `same-range` group. Before, the range was padded to
  `^2.HUGE.HUGE`, as in syncpack, and accepted no version.
- An enum or TypeScript namespace read through a namespace import (`ns.E.A`, `ns.E['A']`, the type `ns.E.A`), also
  through a named or `export * as` re-export, uses only the members read on it, so its other members are reported by
  `unused-enum-members` and `unused-namespace-members`. `ns.E` used as a whole still uses every member
  ([ADR 0083](adrs/0083-nested-namespace-reads-shadowed-imports-and-same-name-type-containers.md)).
- A parameter or local with the name of an import declaration's binding shadows it: passing it around or reading its
  members no longer uses the imported module as a whole. A parameter decorator (`@Inject(tok.KEY) tok`) still reads
  the import, as TypeScript evaluates it outside the function. `import x = require("x")` is still matched by name
  ([ADR 0083](adrs/0083-nested-namespace-reads-shadowed-imports-and-same-name-type-containers.md)).
- An import declaration's binding and the binding of a `const ns = await import("x")` or `require("x")` in a
  function with the same name each use the members read through them, not both every member read through the name
  ([ADR 0083](adrs/0083-nested-namespace-reads-shadowed-imports-and-same-name-type-containers.md)).
- An exported `type` or `interface` that shares its name with a value export (`export const Service = X; export type
  Service = X`, `export const IStore = createDecorator<IStore>(…); export interface IStore { … }`) credits the
  exports it mentions whenever the name is used, as a type or a value, as knip does, also when an `export { Service }`
  clause exports the pair
  ([ADR 0083](adrs/0083-nested-namespace-reads-shadowed-imports-and-same-name-type-containers.md)).
- The types the signatures of an overloaded function mention are used when the function is, from every signature, not
  only the last one.
- The Next.js plugin reads `turbopack.resolveAlias` and `turbopack.rules`, as knip does: a package an alias points to
  (`"react-native": "react-native-web"`, also in a list or a `browser` condition) and the loaders of a rule are used
  dependencies, and an alias to a path (`"@ui": "./src/ui"`) is a module alias.
- A zx file that switches to PowerShell (`usePwsh()`, `usePowerShell()`, `$.shell = 'pwsh'`) runs no binary for a
  cmdlet (`New-Item`, `Remove-Item`) or a default PowerShell alias (`gci`, `ni`), so `unlisted-binaries` no longer
  reports them.
- A package with no source file, entry or script of its own (a manifest that only lists what code elsewhere installs
  it for, ADR 0082) is listed in `incomplete-packages` with the new cause `no-entries` instead of `unmapped-published`,
  which it does not publish ([ADR 0094](adrs/0094-script-directories-and-the-no-entries-cause.md)).
- A `package.json` script command after `cd <dir>` or `pushd <dir>`, or run by `npm --prefix <dir> exec`,
  `pnpm -C <dir> exec` or `yarn --cwd <dir> exec`, runs in that directory: the files it names are relative to it, and in a workspace package
  there the binaries it runs and the packages it loads are credited to that package and never unlisted. knip judges
  them at the script's package, so `cd sub && pnpm exec rspack` no longer reports `rspack` unlisted while `sub` lists
  `@rspack/cli` ([ADR 0094](adrs/0094-script-directories-and-the-no-entries-cause.md)).
- A classic (v1) `yarn.lock` is no longer read for the binaries and peers of packages that are not installed: it
  records no `bin`, so every locked package looked like one without binaries, and the devDependencies whose binaries
  scripts run were reported by `unused-dev-dependencies` (and removed by `--fix-unsafe`). Only a Berry lock, with its
  `__metadata:` block, is read.
- Judging the packages tool configs name decides once per config file whether git ignores it, not once per package
  reading it and package it names: hundreds of packages extending one base `tsconfig` below a long `.gitignore` no
  longer rebuild the ignore matchers thousands of times.
- The commands of MCP servers (`.mcp.json` and the other MCP configs), Tauri's before commands, Travis CI, Railway
  services and mise tasks judge only the binaries they reach through a package manager or a `node_modules/.bin`
  path, as knip's `optionalBinaries` has it for the same plugins: `uvx`, `trunk`, `codecov` or `shellcheck` run by
  name are no longer `unlisted-binaries` findings. The binaries a mise file task runs are reported at the task file,
  not at `package.json`
  ([ADR 0092](adrs/0092-machine-tool-binaries-unread-templates-and-run-time-entries.md)).
- Vitest's built-in `agent`, `html` and `minimal` reporters name no package, so a config using them is no longer an
  `unlisted-dependencies` finding; `html` credits `@vitest/ui`. The `nuxt` environment credits
  `vitest-environment-nuxt` without requiring it, as `@nuxt/test-utils` brings it.
- A `tsconfig` `types` entry that a local `typeRoots` directory of the same config holds (`types/custom/index.d.ts`)
  is no longer an `unlisted-dependencies` finding: TypeScript loads it from there, not from a package.
- An extensionless mise file task whose shebang runs it with `node`, `bun`, `deno`, `tsx` or `ts-node`
  (`mise-tasks/gen` with `#!/usr/bin/env node`) is read as JavaScript: the relative modules it imports or requires are
  entries, so they are no longer `unused-files`. The packages it imports are not credited yet.
- A package that only a module of another workspace package names, which a config imports values from (an Oxlint
  `extends: [base]` of a shared config package, a Stylelint spread of an imported base config), is judged against the
  package owning that module: it is no longer reported unlisted at the importing config, nor unused in the shared
  config package that lists it. `DepRef::from` of `skif-js-plugins` names that module
  ([ADR 0092](adrs/0092-machine-tool-binaries-unread-templates-and-run-time-entries.md)).
- The `vite`, `vitest` and `electron-vite` plugins read the build input of Vite 8 from `build.rolldownOptions.input`,
  before `build.rollupOptions.input`, as knip does: its pages and modules are entries, so their files and the
  packages they import are no longer reported unused, and a computed Rolldown input leaves the package's entries
  incomplete, as a computed Rollup input does.
- A component that `unplugin-vue-components` registers is no longer reported by `unused-files` when the package has a
  `.vue` file whose template skif reads no tags in (`<template lang="pug">`) or whose script is TSX or JSX: such a
  package credits every component it registers, as a package with `.tsx` files does, since a use skif cannot read
  is assumed rather than a used file reported (and deleted by `--allow-remove-files`)
  ([ADR 0092](adrs/0092-machine-tool-binaries-unread-templates-and-run-time-entries.md)).
- A `bin` path that a script of the package may build is no longer reported by `stale-js-config` as naming no file:
  besides build scripts and the compilers and bundlers ADR 0069 lists, a script running `bun build`, `vite build`,
  `deno compile`, `parcel build`, `bunchee`, `microbundle`, `pkg` or `nexe`, or naming a path at or in `bin` as its
  output (`--outfile bin/cli.js`, `-o bin/cli`), is a build step. knip reports no such path either; ADR 0069 said it
  did ([ADR 0092](adrs/0092-machine-tool-binaries-unread-templates-and-run-time-entries.md)).
- A Serverless handler with a variable read when the CLI runs and a fallback (`src/${opt:stage, 'dev'}/b.main`) is
  an entry for every value of the variable (`src/**/b.{js,ts}`), not only its fallback, so the handlers of the other
  stages are no longer `unused-files`. A `${self:…}` variable still takes its fallback
  ([ADR 0092](adrs/0092-machine-tool-binaries-unread-templates-and-run-time-entries.md)).
- `explain-usage` lists a template's use of a registered Vue component without a line (`line` 0 in JSON, "a template
  uses it as a registered component" in text) rather than as line 1 of the template's file, follows an export that an
  `export * from` forwards through the re-exporting module's users rather than calling it no export of that module,
  and describes an `export *` importer as re-exporting every export.
- A Prisma `generator` whose `provider` is the Python or Go client (`prisma-client-py`, `prisma-client-go`) names no
  npm package, so it is no longer an `unlisted-dependencies` finding at the schema.

### Changed

- The parse cache schema is raised to 2, as `rs` and `rstest` mock calls are now mock imports: a `--cache` store
  written by an earlier build is not trusted and is rebuilt on the next run.
- `unlisted-binaries` judges the commands that plugins read from the files of their tools (workflow steps, Git
  hooks, `lint-staged` tasks and other configs that hold commands) and the calls of `execa`, `zx` and `nano-spawn`,
  and reports a binary no dependency provides at the file that runs it. The literal commands of `exec`, `execSync`,
  `spawn`, `spawnSync`, `execFile` and `execFileSync` from `node:child_process` in files reachable from an entry are
  read as well, as knip reads them ([ADR 0076](adrs/0076-binaries-run-outside-scripts-and-unlisted-corrections.md),
  [#97](https://github.com/Goldziher/skif/issues/97)). `ModuleInfo` gains `commands`, `PluginOutput` gains
  `run_binaries`, `PluginInput` gains `SourceScript`, `WorkspaceInputs` gains `run_binaries`, `UnlistedBinary`
  gains `file`, and the module cache schema moves to 2. The commands of CI workflows, Git hooks and task runners
  judge only the binaries they reach through a package manager or a `node_modules/.bin` path, as knip's
  `optionalBinaries` does (`PluginSpec` gains `optional_binaries`, `BinaryUse` gains `bin_path`), and a GitHub
  Actions step whose `shell` is not a POSIX shell is not read.
- In a package whose incomplete entries lie in a narrowed scope, `unused-exports`, `unused-types`, the namespace
  export rules, the member rules and the tag hints of `stale-js-config` now judge every file outside the scope: every
  file the scope matches is an entry, outside the package's `project` globs and in the workspace packages nested
  below it too, so whatever an unknown entry imports is counted. Files in the scope, packages unjudged as a whole and
  the dependency rules are unchanged. A spread above a Vite or Vitest entry key of a value the config computes itself
  (`build: { ...makeLib() }` with `makeLib` declared in the config) now leaves the package unjudged as a whole, as a
  spread inside the key does; only an imported base narrows
  ([ADR 0081](adrs/0081-exports-outside-a-narrowed-incomplete-scope-are-judged.md),
  [#101](https://github.com/Goldziher/skif/issues/101)).
- The `esbuild` plugin reads build scripts that export nothing (`run({ entryPoints: { … } })`,
  `await esbuild.build({ entryPoints: [...] })`, also inside a function, where a local that shadows a top-level
  binding is unknown): each `entryPoints` they write is a production entry, and the package is no longer unjudged as
  unparsable
  ([ADR 0081](adrs/0081-exports-outside-a-narrowed-incomplete-scope-are-judged.md),
  [#101](https://github.com/Goldziher/skif/issues/101)).

### Fixed

- A project whose only JavaScript is a pnpm install hook (`.pnpmfile.cjs`, `.pnpmfile.mjs`) and that has no script
  is no longer judged for unused dependencies: its manifest lists what code elsewhere installs it for, as for a
  project with no source file. The hook is still an entry, so what it imports is used
  ([ADR 0082](adrs/0082-install-hooks-sveltekit-route-types-and-builtin-named-packages.md),
  [#98](https://github.com/Goldziher/skif/issues/98)).
- `unresolved-imports` no longer reports SvelteKit's `./$types` while the `.svelte-kit/tsconfig.json` the project's
  `tsconfig.json` or `jsconfig.json` extends is not generated and is gitignored
  ([ADR 0082](adrs/0082-install-hooks-sveltekit-route-types-and-builtin-named-packages.md),
  [#99](https://github.com/Goldziher/skif/issues/99)).
- `declare module 'pkg/sub'` declares that module only, no longer the whole package: the other imports of `pkg` are
  judged by `unlisted-dependencies` again. `declare module 'pkg'` and a pattern such as `declare module 'pkg/*'` still
  cover the imports they match ([ADR 0086](adrs/0086-packages-tool-configs-name-are-unlisted-at-the-config.md), [#96](https://github.com/Goldziher/skif/issues/96)).
- A specifier that names a workspace package is an import of that package also when a `tsconfig` mapping or bundler
  alias leads it to generated code in that package that is not there yet, as it already was when the mapping leads to
  an existing file; an alias into other code is not the package. An import an alias leads into another package is
  covered by a `declare module` of any module of that package ([ADR 0086](adrs/0086-packages-tool-configs-name-are-unlisted-at-the-config.md), [#96](https://github.com/Goldziher/skif/issues/96)).
- A string `extends` of an ESLint flat config names a plugin's config, no `eslint-config-*` package, and a Prisma
  `generator` block named `client…` is not read, as knip reads them. A config that a sibling package extends through
  `../` is read at its own path ([ADR 0086](adrs/0086-packages-tool-configs-name-are-unlisted-at-the-config.md)).
- Vite's `optimizeDeps.include` and the resolvers of `eslint-plugin-import` settings are optional references, as knip
  reads them: a listed one is never unused and an unlisted one is never reported
  ([ADR 0086](adrs/0086-packages-tool-configs-name-are-unlisted-at-the-config.md)).
- In a multi-root run (`skif -r`), a file another root uses is no longer reported by `unused-files` or deleted by
  `--allow-remove-files`: a file of a nested root that a workflow step or another tool configuration of an enclosing
  root names is an entry of the nested root, and a file a root's code reaches outside its own directory (an import of
  the enclosing root's `.github/actions/detect.ts`, of a sibling root's file, and what those import) counts as used
  by the root that holds it, as do the dependencies such a file imports, in the manifest that lists them. The JS
  analysis follows imports of source files outside the workspace root, which it used to take for assets, as files of
  another project that it does not judge
  ([ADR 0085](adrs/0085-files-other-roots-reach-count-as-used.md),
  [#100](https://github.com/Goldziher/skif/issues/100)).
- The `jest` plugin reads `testRegex` (a string, a regular expression or an array of them): the test files it selects
  are entries besides the default test globs, which stay as knip keeps them. `<rootDir>` stands for the config's
  `rootDir`, and the `i`, `m` and `s` flags of a regular expression literal apply; an expression the `regex` crate
  cannot compile leaves the package's entries incomplete. `testPathIgnorePatterns` is not applied, as knip does not
  apply it ([ADR 0085](adrs/0085-files-other-roots-reach-count-as-used.md),
  [#100](https://github.com/Goldziher/skif/issues/100)).
- The JS analysis walks a hidden directory that holds workspace members, such as `.vscode/extensions/*`, for those
  members, which it used to skip with the rest of the directory; the knip comparison's `skipped-directory` deviation
  follows ([ADR 0085](adrs/0085-files-other-roots-reach-count-as-used.md),
  [#100](https://github.com/Goldziher/skif/issues/100)).
- The `jasmine` framework of a Karma config uses `jasmine-core` as well as `karma-jasmine`, as knip counts it
  ([#98](https://github.com/Goldziher/skif/issues/98)).
- `next lint` in a script uses `eslint` and `eslint-config-next`, which it runs
  ([#98](https://github.com/Goldziher/skif/issues/98)).
- `unresolved-imports` no longer reports a missing file that the `.gitignore` of a directory other than the root or
  a package lists (any directory up to the git repository root, so also one above a nested root), an import of an
  ignored path below another `rootDirs` directory that is not generated yet (React Router's `./+types/root`), or a
  root-relative import of a JSON or other non-code file such as `/vite.svg`
  ([#99](https://github.com/Goldziher/skif/issues/99)).
- The `vercel` plugin is enabled by a `vercel.json` too and parses its `buildCommand`, `devCommand`,
  `installCommand` and `ignoreCommand` as scripts, so `"buildCommand": "pnpm turbo build"` keeps `turbo` used
  ([#98](https://github.com/Goldziher/skif/issues/98)).
- In a multi-root run (`skif -r`) a dependency that only a nested root imports or runs, and that the nested root
  does not list but a `package.json` above it does, is no longer reported unused in that outer manifest: Node.js
  resolves it from the outer `node_modules`. Reports carry these uses in `Report::ancestor_dependency_uses`, and
  `Report::retain` drops diagnostics while keeping the summary right
  ([#98](https://github.com/Goldziher/skif/issues/98), [ADR 0077](adrs/0077-dependencies-nested-roots-use-count-for-the-enclosing-manifest.md)).
- The script parser skips the bodies of here-documents, does not read `case` patterns as commands and does not
  report the functions a script defines as binaries; a command a source file runs follows the scripts of its package
  (`execSync('pnpm build')`); and more system and platform tools (`taskkill`, `xcode-select`, `sysctl`, `osascript`,
  `powershell.exe`, compilers) count as global binaries
  ([ADR 0076](adrs/0076-binaries-run-outside-scripts-and-unlisted-corrections.md)).
- A dependency named like a binary is assumed to provide it only while its manifest is unknown: when the installed
  or locked manifest has no such binary (the `stripe` SDK beside a `stripe listen` script), `unlisted-binaries`
  reports the binary ([#97](https://github.com/Goldziher/skif/issues/97)).
- The `engines` of a package that encloses another one provide their runtime's module there too: the client and
  server packages inside an editor extension that declares `engines.vscode` may import `vscode` without
  `unlisted-dependencies` reporting it ([#96](https://github.com/Goldziher/skif/issues/96)).
- A bare `@import` in a plain `.css` file is looked up beside the stylesheet first, as `postcss-import` and the CSS
  URL rules do, and as a package only when no file there matches: `@import '_code'` next to `_code.css` is no longer
  an unlisted dependency `_code` ([#96](https://github.com/Goldziher/skif/issues/96)).
- A path below `node_modules/.bin` (`require.resolve('.bin/tool')`) or another dot directory of `node_modules`
  names no package, so `unlisted-dependencies` no longer reports `.bin`
  ([#96](https://github.com/Goldziher/skif/issues/96)).
- A `@jest-environment <name>` or `@vitest-environment <name>` pragma in the comments before a file's first
  statement is an import of the environment's package (`jsdom` is `jest-environment-jsdom` for Jest,
  `edge-runtime` is `@edge-runtime/vm` for Vitest, `node` imports nothing, a path stays a path), as knip reads it:
  a listed environment is no longer an unused dependency and an unlisted one is reported by
  `unlisted-dependencies` ([#96](https://github.com/Goldziher/skif/issues/96)).
- `skif migrate --from knip` notes, in a workspace of several packages, that knip merges the tsconfig `paths` of all
  workspaces and skif does not, so an import that only another package's `paths` map is reported as
  `unlisted-dependencies` with the detail `ForeignTsconfigPaths`
  ([ADR 0075](adrs/0075-imports-only-another-packages-tsconfig-paths-map.md),
  [#112](https://github.com/Goldziher/skif/issues/112)).

### Changed

- An import that does not resolve for its package but matches a tsconfig `paths` pattern of another workspace package,
  or of a root tsconfig the package does not follow, with a target that is a source file of the workspace, is still
  reported by `unlisted-dependencies`, now with the detail `ForeignTsconfigPaths`, a message naming the config that
  declares the pattern, a related location at it, and help that suggests the mapping or an import by package name
  instead of a `package.json` entry. An import of a workspace package, of a package a manifest lists or of an installed
  package keeps the `package.json` advice. knip merges the `paths` of every workspace, so these imports surface after
  switching from it ([ADR 0075](adrs/0075-imports-only-another-packages-tsconfig-paths-map.md),
  [#112](https://github.com/Goldziher/skif/issues/112)).
- `skif-js`: `UnlistedImport` is `#[non_exhaustive]` and has the fields `specifier`, `target` (what the import resolved
  to) and `foreign_paths` (a new `ForeignMapping`); code outside the crate can no longer build it or match it
  exhaustively.

### Fixed

- A Playwright `testMatch` regular expression with a `?` quantifier or a character class (`/.*\.e2e\.tsx?/`,
  `/.*\.(spec|test)\.[jt]sx?$/`) becomes a glob of the test files it matches. It used to make every script below the
  project's `testDir` an entry, so no file there was reported by `unused-files`
  ([#100](https://github.com/Goldziher/skif/issues/100)).
- The `jest` plugin makes a file named just `test.js` or `spec.ts` (any script extension) a test entry, as Jest's
  default `testMatch` and knip do ([#100](https://github.com/Goldziher/skif/issues/100)).
- `skif explain-usage` names the entry kind `enclosing-entry-glob` for a file that a plugin of a containing package
  makes an entry (a root `playwright.config.ts` whose `testDir` is in a member) instead of `unknown`
  ([#100](https://github.com/Goldziher/skif/issues/100)).
- `stale-js-config` no longer reports stylesheet extensions (`.css`, `.scss`, `.sass`, `.less`, `.pcss`, `.styl`,
  `.stylus`) in `project` as files skif does not analyse: skif reads their imports, so a migrated knip glob such
  as `src/**/*.{ts,css}!` no longer warns. Other unknown extensions are still reported
  ([#110](https://github.com/Goldziher/skif/issues/110)).
- `const [{ a }, ns] = await Promise.all([import("./x"), import("./y")])` uses only what each element of the array
  pattern reads: `a` of `./x` and the members read through `ns` of `./y`, instead of both modules as a whole, so
  their other exports are reported. An import after a spread element or without an element at its position is
  still used as a whole.
- `vi.spyOn(ns, "a")`, `jest.spyOn(ns, "a")` and `spyOn(ns, "a")` read only member `a` of a namespace import
  instead of using the whole module, so its other exports are reported (under `unused-namespace-exports` and
  `unused-namespace-types` when nothing else imports the module).
- `duplicate-exports` reports `export const b = a` when `a` is an exported constant initialised with another
  binding (`export const a = base`); before, only an `a` exported under its own declaration counted.
- `import { type A } from "./a"; export type { A } from "./a";` with `A` in the signature of a used export of the
  same file no longer reports the re-export of `A` as unused: the signature mentions it through the import, as knip
  counts it.

### Added

- The `[js]` ignore lists (`ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, `ignore-files`,
  `ignore-members`, `ignore-workspaces` and the `extend-` keys, at the top level, per workspace and in overrides) takes
  an inline table `{ name = "...", reason = "..." }` alongside plain strings, so an entry records why it exists
  ([ADR 0074](adrs/0074-reasons-on-js-ignore-entries.md), [#113](https://github.com/Goldziher/skif/issues/113)).
- `[lint] require-ignore-reasons = true` turns on the `--require-ignore-reasons` check from the file
  ([#113](https://github.com/Goldziher/skif/issues/113)).
- `skif migrate` turns the comment above a knip ignore entry, or after it on its line, into the entry's `reason` (also
  in a list written as a top-level constant), and writes the comment above a syncpack group that no spread precedes
  above its `[[version-groups]]`, `[[semver-groups]]` or `[[update-groups]]` entry
  ([#113](https://github.com/Goldziher/skif/issues/113)).

### Changed

- **Breaking** for library consumers: the `[js]` ignore-list fields of `skif_config::Js`, `JsWorkspace` and
  `PackageJs` change from `Vec<String>` to `Vec<IgnoreItem>` (a `name` and an optional `reason`); an `IgnoreItem`
  compares with a string by its name and converts from one, and code that reads an entry as a string reads `name`
  ([#113](https://github.com/Goldziher/skif/issues/113)).
- **Breaking** for runs with `--require-ignore-reasons`: the flag now also fails on an `[[ignore]]` entry of an
  override and on an entry of a `[js]` ignore list without a `reason`, so a configuration that passed before can fail
  with exit code 2. The message names the list and the entry, by its place in the merged list
  ([#113](https://github.com/Goldziher/skif/issues/113)).

### Added

- **`stale-mocks` rule** (js, error): reports a `vi.mock`, `vi.doMock`, `vi.unmock`, `vi.importActual`, `jest.mock`,
  `jest.doMock`, `jest.requireActual` or other test-runner call whose relative or aliased specifier names a module that
  does not exist ("`` `vi.mock('./gone')` names a module that does not exist ``"), including Vitest's typed
  `vi.mock(import('./gone'))`. The specifier resolves as an import does, so a query such as `?react` is dropped
  first; a package name that resolves to nothing is not reported, nor is a `{ virtual: true }` mock.
  `ignore-unresolved` and `ignore-dependencies` silence it. knip does not read mock specifiers, so
  `skif migrate --profile exact` turns it off. There are 43 rules, 18 of them `js` rules
  ([ADR 0073](adrs/0073-stale-mocks-test-runner-mocks-of-missing-modules-under-their-own-rule.md),
  [#111](https://github.com/Goldziher/skif/issues/111)).

### Changed

- `unresolved-imports` no longer reports test-runner mocks; a mock of a missing module is a `stale-mocks` error
  instead, with the same default severity, so it can be triaged, baselined or turned off apart from broken imports
  ([#111](https://github.com/Goldziher/skif/issues/111)).
- **Breaking** for library users of `skif-js-syntax`: `Import` has a new public field `call` (an `Option<MockCall>`,
  the test-runner call of a mock import), so code that builds an `Import` with a struct literal must set it. The
  module cache schema is raised with it, so a cache written by an earlier build is discarded.

### Changed

- A `[policy] dependency-types` list that does not select `local` (`["prod", "dev"]`, `["!local"]`) leaves the own
  versions of workspace packages out, as `--dependency-types prod,dev` does in syncpack: a dependent is no longer
  reported for differing from the `version` of the workspace package it names. Without a list nothing changes; add
  `local` to a list to keep the comparison
  ([ADR 0072](adrs/0072-a-policy-scope-without-local-leaves-the-own-versions-out.md)).

### Fixed

- `skif migrate` writes `[policy] dependency-types = ["prod", "dev"]` for sherif, so a migrated sherif project is not
  reported for a dependency that differs from a workspace package's own version, which sherif never compares; with
  syncpack the list is the union of both scopes. A sherif that ignores `multiple-dependency-versions` writes no scope.
- `skif migrate` drops the `./` prefix of sherif's `ignorePackage` paths, which never matched a package in skif.
- `skif migrate` cuts each syncpack group's `dependencyTypes` to the `--dependency-types` of the lint script, and drops
  a group left with none, since syncpack never gave it an instance.
- `skif migrate` unites syncpack scripts that lint different `--dependency-types`, `!` negations included, by what they
  select (`!dev,!peer` and `!peer,!local` give `["!peer"]`); when no list selects what they select together, it writes
  the default scope and leaves the groups' `dependencyTypes` as they are.
- `skif migrate` no longer counts `"ci": "pnpm run knip"` next to a `knip` script as a second run of knip without
  flags, reads `node ./node_modules/knip/bin/knip.js`, and takes `--production=false` as off.
- `skif migrate` lets a syncpack `policy` win over `snapTo` as syncpack does, notes a `preferVersion` it drops under
  `policy: sameMinor`, treats `dependencies` and `packages` of `["**"]` as selecting everything, and marks the
  `policy: catalog` translation approximate, naming how `[catalog] mode = "prefer"` is weaker.

### Added

- `skif baseline write|prune|check` maintains a suppression baseline for a repository that adopts skif with findings it
  cannot fix at once. `write` records every error and warning as an `[[ignore]]` entry (rule, package and dependency,
  with `name@specifier` for version findings so only that version is silenced) in `skif.baseline.toml` (`--file`),
  each with `reason = "baseline"` (`--reason`), and adds the file to the `extends` of the root's `skif.toml` without
  changing the rest of it. `prune` removes the entries that match no finding, and `check --base <REF>` (or
  `--base-file <FILE>`) exits `1` when the baseline gained entries. A finding in a root package without a name of
  its own is not recorded, since no package pattern selects the root alone. All three take `--format json`
  ([ADR 0071](adrs/0071-skif-baseline-a-suppression-baseline-that-can-only-shrink.md),
  [#114](https://github.com/Goldziher/skif/issues/114)).
- `ignore_matches` in the library returns the findings each `[[ignore]]` entry matches, judged by the code a lint run
  suppresses with.
- A published path that names nothing (a `bin` such as `bin/cli.js` with no file, no source behind it, outside a build
  output directory, not ignored by git and with no build step of the package that may write it) no longer marks its
  package `unmapped-published` and leaves it unjudged: it is no entry, the package is judged from its other entries,
  and `stale-js-config` reports the path (`package-entry`). Ignore files above the analysed root and
  `.git/info/exclude` now count when telling build output from such a path
  ([ADR 0069](adrs/0069-a-published-path-into-nothing-is-reported-not-an-incomplete-entry.md),
  [#109](https://github.com/Goldziher/skif/issues/109)).
- A spread of a conditional object literal in a config (`...(tmp && { cacheDir: tmp })`, `...(ci ? { … } : {})`) no
  longer marks the package incomplete with `unknown-spread`, which kept `unused-dependencies` and the other package
  rules from judging it. Every branch is read: the lists each branch sets (`include`, `setupFiles`, …) are credited,
  and only a branch that is not known statically leaves the spread unknown
  ([#108](https://github.com/Goldziher/skif/issues/108)).
- The GitHub Action, pre-commit and Poly examples in the README and the docs pin the 0.2 line (`Goldziher/skif@v0.2`,
  `v0.2.0`) instead of 0.1. `scripts/release-bump.sh` now rewrites these pins on every final release, and
  `scripts/check-doc-pins.sh` fails CI when one is not on the workspace version's release line
  ([#94](https://github.com/Goldziher/skif/issues/94)).

### Added

- Windows npm packages: `skif-win32-x64-msvc` and `skif-win32-arm64-msvc`, so `npx skif-cli`, `pnpm dlx skif-cli`
  and `bunx -p skif-cli skif` run on Windows x64 and arm64. The release loads both addons on their own architecture
  under Node.js 22, 24 and 26 and Bun before publishing ([ADR 0070](adrs/0070-windows-npm-packages.md),
  [#93](https://github.com/Goldziher/skif/issues/93)).

## [0.2.0] - 2026-11-08

### Added

- `--cache` keeps the parse results of source files between runs in `node_modules/.cache/skif/modules.bin`, and
  `--cache-location <DIR>` moves the store; the cache is keyed by each file's name and text and never changes the
  report. `--watch`, `skif lsp` and `skif mcp` keep an in-memory cache for as long as they run. Library hosts pass a
  `ModuleCache` to `Linter::with_module_cache`; `MemoryModuleCache` is the in-process one
  ([ADR 0066](adrs/0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md),
  [#44](https://github.com/Goldziher/skif/issues/44)).
- `skif lsp` lints the editor's unsaved buffers: it takes the full text on every change, lints a project 250
  milliseconds after its last change, and offers fixes for unsaved documents at positions in the buffer, with the
  document version in `documentChanges` when the client supports it. Library hosts overlay buffers on a file system
  with `BufferFs` ([#44](https://github.com/Goldziher/skif/issues/44), [ADR 0066](adrs/0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md)).
- `vue`, `mdx`, `next-mdx`, `pre-commit`, `parcel`, `electron-forge`, `unplugin-auto-import`,
  `unplugin-vue-components`, `unplugin-vue-router`, `github-action`, `pino`, `next-intl`, `dependency-cruiser`,
  `commitizen`, `serverless-framework`, `starlight`, `execa`, `zx`, `nano-spawn`, `azure-pipelines` and `sapper`
  plugins ([ADR 0059](adrs/0059-twenty-one-more-plugins-and-commands-run-from-source.md),
  [#39](https://github.com/Goldziher/skif/issues/39)). The table holds 127 plugins.
- The command of `glob -c "<command>" <patterns>` in a script is parsed as a script
  ([#39](https://github.com/Goldziher/skif/issues/39)).
- A plugin's `package.json` key may be nested (`config.commitizen`, `config.forge`).
- `tauri`, `capacitor`, `convex`, `sst`, `payload`, `qwik`, `quasar`, `wxt`, `stencil`, `eleventy`, `cucumber`,
  `hardhat`, `temporal`, `relay`, `react-email`, `openapi-ts`, `orval`, `danger`, `xo`, `textlint`, `taskfile`, `mise`,
  `moonrepo`, `wireit`, `travis`, `buf`, `vite-plugin-pwa`, `nuxtjs-i18n`, `rstest`, `sveltejs-package`,
  `playwright-ct`, `fumadocs`, `purgecss`, `nano-staged` and `changelogen` plugins
  ([ADR 0060](adrs/0060-thirty-five-more-plugins-frameworks-task-runners-and-options-written-anywhere.md),
  [#40](https://github.com/Goldziher/skif/issues/40)). The table holds 162 plugins. `sst` also reads the handlers of
  the modules `sst.config.ts` imports, such as `infra/*`.
- `skif-js-syntax` extracts the values of a property wherever it is written in a file (`extract_property_values`).
- `astro-db`, `astro-markdoc`, `astro-og-canvas`, `borp`, `bumpp`, `changelogithub`, `create-typescript-app`, `eve`,
  `expressive-code`, `filesystem-routing`, `flue`, `kysely-codegen`, `ladle`, `laravel-vite-plugin`, `linthtml`,
  `lockfile-lint`, `lost-pixel`, `lunaria`, `marko`, `mcp`, `mdxlint`, `node-modules-inspector`,
  `npm-package-json-lint`, `openclaw`, `osls`, `playwright-test`, `railway`, `raycast`, `react-cosmos`,
  `solidjs-vite-plugin`, `tsrs`, `typescript-content-mapper`, `varlock`, `vercel-og`, `vite-plugin-pages`,
  `vite-plugin-vue-layouts-next`, `vite-plus`, `vite-pwa-assets-generator` and `yorkie` plugins
  ([ADR 0061](adrs/0061-thirty-nine-more-plugins-and-plugins-enabled-by-a-package-json-key.md),
  [#41](https://github.com/Goldziher/skif/issues/41)). The table holds 201 plugins. `mcp` reads the commands of the
  servers in `.mcp.json`, `.cursor/mcp.json`, `.vscode/mcp.json` and `.codex/config.toml`, and `varlock` the
  `@plugin` and `@import` decorators of `.env` files.
- A plugin can be enabled by a key of the workspace's `package.json` (`Enabler::ManifestKey`).
- An exported class registered as a custom element is no unused export: `customElements.define("x-y", Class)`
  always counts, and the registering decorator or base class of an enabled plugin's library counts too
  ([ADR 0062](adrs/0062-custom-element-registrations-and-the-taskr-plugin.md),
  [#38](https://github.com/Goldziher/skif/issues/38)). `stencil` now marks `@Component` classes.
- `lit`, `catalyst` and `fast` plugins, which mark the classes their decorators (`@customElement`, `@controller`) or
  `FASTElement.define()` register ([#39](https://github.com/Goldziher/skif/issues/39),
  [#41](https://github.com/Goldziher/skif/issues/41)), and a `taskr` plugin: `taskfile.js` and the local plugins of
  `package.json#taskr.requires` are entries, and `@taskr/*` and `taskr-*` dependencies are used
  ([#20](https://github.com/Goldziher/skif/issues/20)). The table holds 205 plugins.
- `skif list` and `skif json` show every dependency instance the version groups and the catalog govern, with its
  version group, specifier and state, as syncpack's `list` and `json` do; `json` prints one object per line with
  syncpack's `statusCode` names ([#35](https://github.com/Goldziher/skif/issues/35)).
- `--dry-run` with `--fix` or `--fix-unsafe` prints the planned changes as a unified diff, writes no manifest and runs
  no install (reports to `--output` or `FORMAT=FILE` files are still written); with `--format json` the `fix` object
  carries the plan's `changes` and their diffs ([#35](https://github.com/Goldziher/skif/issues/35)).
- `--dependency`, `--dependency-types`, `--specifier-types` and `--source` narrow the version-group and semver-group
  rules (and only those) of a lint run, `skif list` and `skif json` to the instances they select, as syncpack's
  filters do; `--dependency` is syncpack's `--dependencies`, renamed because knip's `--dependencies` shortcut belongs
  to the same command ([#35](https://github.com/Goldziher/skif/issues/35)).
- The `semver-range-only` version-group policy: versions may differ and only the range the semver groups require is
  checked and fixed. `skif migrate` writes it for syncpack's `semverRangeOnly`, which it used to turn into `ignored`,
  dropping the range check ([#35](https://github.com/Goldziher/skif/issues/35)).
- The `name~version` custom type strategy reads the dependency name from `name-path` and the version from `path`;
  `skif migrate` translates syncpack's `name~version` custom types ([#35](https://github.com/Goldziher/skif/issues/35)).
- The `packages` of `lerna.json` declare the workspace members when nothing else does, and its `npmClient` names the
  package manager, as syncpack reads them; the file is read only then, and an invalid one declares nothing
  ([#35](https://github.com/Goldziher/skif/issues/35)).
- [ADR 0057](adrs/0057-syncpack-list-json-dry-run-filters-semver-range-only-name-version-and-lerna.md) records these
  additions and what of syncpack stays out: per-group and per-state severity, `dependencyGroups`, a scoped `catalog`
  policy, custom type `source`, catalog entries as instances, `.x` ranges and `--log-levels`.
- `skif update [--check] [--target latest|minor|patch] [--dependencies <NAME>]`: newer published versions from the npm
  registry, written format-preserving or, with `--check`, reported with exit `1`, as syncpack's `update`
  ([#34](https://github.com/Goldziher/skif/issues/34),
  [ADR 0058](adrs/0058-registry-updates-an-opt-in-skif-update-command.md)). It is the only command that uses the
  network; linting never does. It reads the registry and tokens of `.npmrc` (the project's `.npmrc` expands an
  environment variable only in a token, which is then only sent over https to `registry.npmjs.org` or a host the user's
  own configuration names; a registry URL there that reads one is ignored), honours
  `[update] minimum-release-age` (else pnpm's `minimumReleaseAge`, else one day; a version without a publish time is
  then not proposed) and `[[update-groups]]`, and reports each instance with syncpack's `DiffersToNpmRegistry` state.
  The library plans updates through a `Registry` trait the host implements (`skif::update`, feature `update`).
  `skif migrate` translates syncpack's `updateGroups`, `minimumReleaseAge` and `maxConcurrentRequests`.
- `skif update --interactive`: syncpack's interactive update picker. A terminal list shows each dependency once per
  current and target specifier, with the instances it moves, and only the chosen updates are written. It needs a
  terminal on stdin and stdout and cannot be combined with `--check` ([#34](https://github.com/Goldziher/skif/issues/34)).
- knip's run-time filters on the lint command ([#36](https://github.com/Goldziher/skif/issues/36)):
  - `--workspace <NAME|DIR>` (`-W`) reports only the findings of the selected workspace packages; findings of no
    package belong to the root.
  - `--strict` is `--production` with each workspace package limited to the dependencies its own manifest lists.
    As in knip, production code must import from `dependencies` or `peerDependencies`, type-only imports use no
    package, and `unused-dependencies` also reports required peer dependencies nothing imports.
  - `--include` and `--exclude` take knip issue types or skif rule ids, with the shortcuts `--dependencies`,
    `--exports`, `--files` and `--cycles`; `--include` turns on a rule that is off; `--tags -TAG` leaves out tagged
    exports.
  - `--max-issues <N>`, `--max-show-issues <N>` (human and JSON output, not with `--fix`), `--no-exit-code` and
    `--include-entry-exports`.
  - `--no-gitignore` analyses the files `.gitignore` hides.
  - `--tsconfig <FILE>` (also `--tsConfig`, `-t`) and `[js] tsconfig` name the root package's TypeScript
    configuration in place of its `tsconfig.json`; `--use-tsconfig-files` and `[js] use-tsconfig-files` take each
    package's project files from its `tsconfig.json` (`files`, and `include` minus `exclude`). A `tsconfig` path must
    exist and stay inside the root wherever JavaScript analysis runs. `skif migrate` maps knip's `--tsConfig` and
    `--use-tsconfig-files`, and notes scripts that pass different `--tsConfig` values.
  - `--trace-dependency <NAME>` prints where a dependency is listed, imported and used.
  - `--debug`, `--performance`, `--duration` and `--memory` print what was loaded, phase timings, wall time and peak
    memory to stderr.
- `skif-bench deadcode` (`mise run bench:deadcode`) measures dead-code precision and recall of skif and the pinned
  knip against a generated workspace with seeded dead code, live code behind the patterns that caused false findings,
  and an answer key ([#26](https://github.com/Goldziher/skif/issues/26)).
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
- `skif lsp` answers `textDocument/codeAction`: each fix of a diagnostic under the cursor is a quick fix (an unsafe
  one is labelled `(unsafe)`), and `source.fixAll.skif` applies every safe fix of the file. The edits come from the
  same fix plan as `skif --fix --dry-run`; fixes that create or delete files, and fixes of a file with unsaved
  changes, are not offered ([#44](https://github.com/Goldziher/skif/issues/44)).
- `ConfigLoader::with_sequential` makes `add_package_files` probe the member directories on the calling thread, for a
  host that runs with one thread; by default they are probed on the current rayon pool
  ([#29](https://github.com/Goldziher/skif/issues/29)).

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
- `skif-bench run --install` installs the corpus before timing knip, which otherwise fails to load create-t3-turbo's
  tool configurations; `mise run bench:publish` passes it ([#27](https://github.com/Goldziher/skif/issues/27)).
- `corpus.toml` takes a per-repository `entry` list that the knip comparison gives both tools; vscode lists the entry
  points its bundler names, so the 1,030 workbench files both tools reported as unused are no longer counted as
  agreement ([#27](https://github.com/Goldziher/skif/issues/27)).
- The knip comparison runs skif recursively on `vercel-examples`, `mastra` and `next-js`, whose examples, templates
  and test apps are nested roots knip reads from the root ([#27](https://github.com/Goldziher/skif/issues/27)).
- The knip comparison counts knip-only exports, types, namespace exports and types, and enum and namespace members in
  a package skif lists under `incomplete-packages` as `not-judged-by-skif`, as it did for files and dependencies:
  skif does not report them there either
  ([ADR 0068](adrs/0068-what-the-knip-comparison-sets-apart.md), [#27](https://github.com/Goldziher/skif/issues/27)).
- The knip comparison records the configuration files knip could not load (`ERROR: Error loading`, exit 2) in
  `parity.json`, their number in the baseline and `parity.md`, and counts knip-only findings in their packages as the
  `knip-config-error` deviation instead of as differences; create-t3-turbo and vue-core have such files
  ([ADR 0068](adrs/0068-what-the-knip-comparison-sets-apart.md), [#27](https://github.com/Goldziher/skif/issues/27)).
- The `fixture-file` deviation of the knip comparison covers only `fixtures` and `__fixtures__`, the directories skif
  does not report; knip-only files below `fixture`, `testdata`, `test_root`, `__testfixtures__` or `*-fixtures`, which
  skif judges, count as differences unless they are test files
  ([ADR 0068](adrs/0068-what-the-knip-comparison-sets-apart.md), [#27](https://github.com/Goldziher/skif/issues/27)).
- The parity baseline is refreshed for create-t3-turbo, vue-core, tanstack-query, jest, babel, cal-com, mastra,
  vscode, npm-cli and rushstack, and the README's agreement with knip is measured on the nine of them compared. The
  old figures counted babel's fixture files, which skif no longer reports, as shared; n8n, angular, opencode,
  rules-js, vercel-examples and next-js keep their earlier entries until a refresh
  ([#27](https://github.com/Goldziher/skif/issues/27)).
- The `test-file` deviation of the knip comparison no longer claims why skif has no finding there: next-js's thousands
  are app files of test fixtures below test directories, not test entries, and the harness sets them apart by path
  ([#27](https://github.com/Goldziher/skif/issues/27)).
- `parity --install` compares a repository with `install = false` uninstalled instead of skipping it before either
  tool runs, so rushstack's entry names knip's own error (`Unable to find package.json`); npm-cli installs with npm 11,
  since npm 10 rejects its lockfile ([#27](https://github.com/Goldziher/skif/issues/27)).
- Manifest checks are faster on large workspaces: about 2.5 times faster on a generated 2,000-package workspace with
  tens of thousands of findings, and twice as fast at 500 packages. Package configuration files are probed in
  parallel, the version-group analysis and its findings run in parallel, the JSON report is written without the
  generic serializer and streamed to stdout when stdout is the only output, and the `skif` binary no longer frees its
  results one by one before exiting (the npm package, which runs in a long-lived Node process, still does). The
  output is unchanged ([#29](https://github.com/Goldziher/skif/issues/29)).
- Manifest checks beat sherif at every size of the generated benchmark: at 2,000 packages `skif --no-js` takes about
  101 ms where it took 122 to 126 ms, against sherif's 125 to 130 ms (1.3 times faster), and at 500 packages 27 ms
  instead of 31 to 34 ms. A report is sorted on indices with each file path ranked once; the policy findings reach the
  emitter, and the providers' diagnostics are merged, without copying the vectors (`Emitter::emit_all`,
  `Emitter::merge_all`); the policy rules look instances up in hash maps, borrow dependency names, and collect their
  instances while the catalog usage index is built (`skif_workspace::join`); the JSON report renders each file's path
  once; and manifests written as strict JSON are read in one pass, without `jsonc-parser`'s syntax tree (any other
  manifest is read as before). The output is unchanged ([#29](https://github.com/Goldziher/skif/issues/29)).
- A tool configuration skif cannot fully read no longer hides every unused file of its package: only the files the
  configuration could make entries stay unjudged (the `next` router directories, Vite's `index` or `main` module, and
  the tests, test directories and setup modules of `vitest` and `playwright`), and they count as entries. Every other
  file is judged, so dead files such as cal-com's `lib/cropImage.ts` and n8n's `useParentScroll.ts` are reported. An
  unparsable config, a config that computes its own entry keys or is unknown as a whole (except `next`), unmapped
  published output, a plugin without a scope and a computed `import()` in the package keep the whole package
  unjudged. Unused dependencies and exports of such a package are still not judged. `explain-usage` tells the narrowed scope from the package-wide case (the `entry`
  `incomplete-scope`, `incomplete-scope` and `widened-by`), and the human note says the packages were not fully
  checked ([ADR 0065](adrs/0065-narrowed-scope-for-incomplete-entries.md),
  [#23](https://github.com/Goldziher/skif/issues/23)).

### Fixed

- On Windows, `skif lsp` finds the fixes of an open document and lints its unsaved changes, and `--watch` leaves out
  the reports it writes: both resolved those files to `\\?\` verbatim paths, which never matched the paths of a lint.
  The `--use-tsconfig-files` warning prints its path with `/` separators there.
- The `stryker` plugin reads the runner config named by `vitest.configFile` or `jest.configFile` with that runner's
  plugin, so `vitest.stryker.config.ts` is a config file instead of an unused file
  ([#23](https://github.com/Goldziher/skif/issues/23)).
- The GitHub Action's new `from-sherif` input (`true` or `false`, default `false`) is a migration aid: with `true`,
  a project that has no `skif.toml` yet is linted with the settings of its sherif configuration. The
  `sherif` key and the flags of a root script that runs sherif (`sherif -i react -r unordered-dependencies`) are
  translated by `skif migrate --from sherif --profile full` into a temporary file passed as `--config`; the
  skif binary still never reads sherif configuration (ADR 0039). The `skif migrate` note for sherif's `-i name@version` says
  that the ignored version still counts when skif picks the version to align on
  ([#45](https://github.com/Goldziher/skif/issues/45)).
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
- A `/*` that no `*/` follows, such as one in JSX text (`<code>dist/*</code>`) or a regular expression, no longer
  makes an `.mdx` `export` paragraph swallow the rest of the document, or a `.tsrx` file hide the imports after it
  ([#38](https://github.com/Goldziher/skif/issues/38)).
- The `.mdx` ESM scan tells JSX text and regular expressions from code, so `//` or `/*` in them
  (`<a>http://x</a>`, `/:\/\//`) no longer hides a closing bracket and makes an `export` paragraph swallow the rest of
  the document ([#38](https://github.com/Goldziher/skif/issues/38)).
- Inline code in `.mdx` prose follows CommonMark more closely: a backslash-escaped backtick opens no code span, an
  unpaired backtick does not pair with one past a blank line, a template literal in a `{}` expression is not code
  (nor one nested in its `${}`), and a stray `{` in prose does not reach past its paragraph, so the components and members they would have hidden count as used
  ([#38](https://github.com/Goldziher/skif/issues/38)).
- Stencil's styles per mode (`styleUrls: { ios: 'a.ios.css', md: ['a.md.css'] }`) are followed like a `styleUrls`
  array, so the packages those stylesheets import are no longer unused
  ([#38](https://github.com/Goldziher/skif/issues/38)).

Repositories with nested roots ([#19](https://github.com/Goldziher/skif/issues/19)):

- A file of a nested project (a directory with its own `package.json`, such as a nested root) that the outer root
  reaches is not reported in the outer root: its imports are no longer unlisted there and no longer count as uses of
  the outer root's dependencies. The nested root judges the file against its own manifest in its own run.
- A file that a script of a `package.json` above a nested root runs (`node build/npm/postinstall.ts`) is an entry
  of that root, so it and what it imports are no longer reported as unused files.
- In a multi-root run (`skif -r`) a file of a nested root that an enclosing root imports (`build/lib/x.ts`, imported
  from the outer `src`) is an entry of the nested root, so it and what it imports are no longer reported as unused
  files. Nested roots are linted after the roots that enclose them; a root linted on its own still cannot see what an
  outer root imports.
- The `vite` and `vitest` plugins honour Vite's `root`: with `root: "src"` the scripts of `src/index.html` are entries
  and `/`-rooted script sources and HTML inputs start at `src`. A `root` that is not a static string leaves the
  package's entries incomplete.
- After `skif -r --fix` the lint of the fixed tree judges each root as the first lint did: a standalone package below
  another root is again not asked for a `packageManager`, and a member that is a workspace of its own is again left
  to its own root instead of being linted twice.
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
- A tool config that imports a base config from another file of the project or from a workspace package (a vite
  config merging `@scope/vitest-config` with `mergeConfig`, an ESLint config listing a shared flat config) evaluates
  that module statically instead of treating the value as unknown, so the package's entry files are known and its dead
  files are reported. `mergeConfig` keeps a key unknown when either side of it is, and ESLint's `defineConfig` and
  `tseslint.config` list all their arguments. In every config function, not only an imported one, a parameter the
  call does not pass takes its default (`(options = {}) => …` called without arguments sees `{}`) instead of being
  unknown ([#23](https://github.com/Goldziher/skif/issues/23)).
- Config evaluation follows conditionals: `cond ? a : b`, `&&`, `||` and `??` whose test is known, `!x` and `.length` of
  a known string or array, and object pattern parameters with defaults (`({ pinCjs = [] } = {}) => …`). With an unknown
  test, two object branches give the keys both have, a key only one sets being unknown, instead of an unknown spread.
  An `if (test) return a;` before a function's last `return` is followed the same way, instead of being skipped for
  the last `return`. A `const` or `let` declared in a block does not replace a binding outside it, and a local or
  top-level constant written by code that may or may not run is unknown after it instead of keeping its earlier value.
  An object changed through another name for it (`const a = cfg`, `{ test: t }`, a function given `cfg`), by a function
  that writes what it captures (a `forEach` callback), by `delete`, inside an expression, or as `module.exports` after
  the assignment is partly unknown too. An `if` with an unknown test and no `return` leaves a local what both branches
  have in common, so `if (CI) config.reporters = […]` leaves only `reporters` unknown.
  A shared vitest config that adds `resolve` only when `pinCjs` is given no longer leaves its users' entry files
  unknown ([#23](https://github.com/Goldziher/skif/issues/23)).
- The source files a generated module may import (a missing file a `.gitignore` lists, such as an index written by a
  build step) are reached and their exports count as used: those beside it, or for the ignored index of a directory,
  those of the directory holding it and below, never the whole package or its `src`. n8n's migrations, listed only by a
  generated `migrations/sqlite/index.ts`, are no longer unused ([#23](https://github.com/Goldziher/skif/issues/23)).
- The vitest and jest plugins mark their entries incomplete when a key they read files from (`setupFiles`, `include`,
  `testMatch`, `projects`) is unknown below the top level, instead of reading it as empty
  ([#23](https://github.com/Goldziher/skif/issues/23)).
- A computed `import()` such as `` import(`./reporters/${name}.js`) `` also reaches the `.ts` and `.tsx` sources of the
  `.js` files its glob names ([#23](https://github.com/Goldziher/skif/issues/23)).
- Without an install, the peer dependencies and binaries of a workspace package are read from its manifest, so a
  package that lists peers for a workspace dependency (`@testing-library/jest-dom` for a shared vitest setup) is not
  told they are unused, and a script that runs a binary of a workspace package it depends on counts that dependency as
  used ([#23](https://github.com/Goldziher/skif/issues/23)).
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
  `stale-js-config`, and `ignoreGlobalBinaries: true` is noted as skif's default
  ([#43](https://github.com/Goldziher/skif/issues/43)).
- `stale-js-config` reports the rest of knip's configuration hints: redundant `entry` and `project` patterns
  (`entry-redundant`, `project-redundant`), a top-level `entry` or `project` in a workspace with several packages
  (`*-top-level`), `project` extensions skif does not analyse or component extensions it leaves out
  (`project-extension-*`), published `package.json` paths that name no file (`package-entry`, build output and
  gitignored paths excepted) and many unused files with knip's thresholds (`top-level-unconfigured`,
  `workspace-unconfigured`) ([#43](https://github.com/Goldziher/skif/issues/43)).
- `[js] ignore-global-binaries` (default `true`): with `false`, `unlisted-binaries` reports the system tools scripts
  run (`rm`, `curl`, `git`) that no dependency provides; `skif migrate` maps knip's `ignoreGlobalBinaries`
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
- The knip comparison installs `rules-js` with the pnpm its `MODULE.bazel` pins (pnpm 9 rejected the lockfile), a
  failed install is reported with its error line instead of the first npm warning, and the corpus notes say why
  `rushstack` cannot be compared with knip ([#27](https://github.com/Goldziher/skif/issues/27)).
- The `test-file` deviation of the knip comparison says why skif has no finding there: most are apps and helpers below
  test directories that are outside skif's graph, not test entries ([#27](https://github.com/Goldziher/skif/issues/27)).
- `mise run bench:publish` writes a `summary.md` that passes `format:check` ([#27](https://github.com/Goldziher/skif/issues/27)).
- `skif-bench run --out` takes a relative directory, as `mise run bench:publish` passes it: hyperfine runs in each
  target's directory and could not create its export there ([#27](https://github.com/Goldziher/skif/issues/27)).
- `skif-bench run` reads the exports of hyperfine 2 as well as 1, and notes a tool that exits with an error (knip's
  2 when a configuration file does not load) under its table and in the knip check
  ([#27](https://github.com/Goldziher/skif/issues/27)).
- The timed knip run on npm-cli reads the workspaces (`--no-gitignore`, from the new `bench-knip-args` of
  `corpus.toml`), so it compares like for like with skif; npm's `.gitignore` hid them and knip analysed only the
  root package ([#30](https://github.com/Goldziher/skif/issues/30)).
- The published benchmark numbers are from a run on an idle machine, with knip timed on an installed corpus
  ([#27](https://github.com/Goldziher/skif/issues/27), [#29](https://github.com/Goldziher/skif/issues/29),
  [#30](https://github.com/Goldziher/skif/issues/30)).
- An `import.meta.glob` whose options read the matched files as text (`query: '?raw'`, `query: { raw: '' }`, legacy
  `as: 'raw'`) and an import of a `?raw` specifier no longer keep the files in use: they are not loaded as modules.
  Other queries (`?url`, `?inline`, `?worker`) and options that are not static still count as uses. n8n's components
  matched only by `import.meta.glob('../../**/*.vue', { query: '?raw' })` in a test are now unused.
  `skif-js-syntax` reports such a glob as `ImportKind::SourceGlob`. Part of
  [#23](https://github.com/Goldziher/skif/issues/23).

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
