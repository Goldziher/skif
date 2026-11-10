---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0068"]
---

# knip Workspaces, Skipped Directories and Published Sources in the knip Comparison

## Context and Problem Statement

Adjudicating the one-sided findings of the knip comparison per cluster (GitHub issues #96 to #100, under #27) showed
that most of what remains one-sided is not disagreement about the code but about which code each tool looks at:

* knip has one notion of a project, its workspaces: the root and the members the root's workspace patterns select.
  A directory with its own `package.json` that is not a member (next.js and vercel examples, mastra templates,
  vendored packages, angular integration projects) is counted to the enclosing workspace. knip gives its files none of
  that manifest's entry files, plugins or dependencies, so it reports them as unused files and their imports as
  unlisted dependencies of the enclosing workspace, and it never judges that manifest's dependencies or scripts. skif's
  file discovery skips such a directory, and `--recursive` lints it as a root of its own
  ([ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md)). Over 1,000 of skif's unused
  dependencies, about 4,900 of knip's unused files and about 1,450 of its unlisted dependencies on the corpus are of
  this kind.
* skif's file discovery skips hidden directories (except tool ones such as `.github`) and build output by name;
  knip reads files there (`.eslint-plugin-local`, `.ng-dev`, `.claude`, `.vercel/output`).
* skif makes the TypeScript source beside a published path an entry (`./schematics/*.js` publishes
  `schematics/**/index.ts`); knip maps a published path to source only through a tsconfig `outDir`, so it reports
  those sources as unused (angular has about 240).
* [ADR 0068](0068-what-the-knip-comparison-sets-apart.md) limits `fixture-file` to unused files, while skif reports
  no export, type or member in a fixture directory either, and no unresolved import or unlisted dependency
  ([ADR 0050](0050-fixture-directories-in-the-js-analysis.md)).

Which of these can the comparison set apart without listing corpus findings, and how does it decide membership?

## Decision Drivers

* A one-sided count must mean "unexplained", so findings that follow from where a tool looks must not land in it
* Membership in a category must be decided from facts the harness can check (paths, manifests, the two reports),
  never from a list of corpus keys
* Each category must say what it checks and what it does not establish
* Baselines hold counts only, never names from the compared repositories

## Considered Options

* Four new path and manifest categories and a wider `fixture-file`
* One `nested-root` category for every finding below a non-member project, on both sides and in every issue type
* Leave the counts as they are and explain the clusters in the documentation

## Decision Outcome

Chosen option: "Four new path and manifest categories and a wider `fixture-file`", because each rule checks one fact
the harness can read, and each covers only the issue types and the side where that fact explains the finding.

**knip's workspaces.** The harness reads the workspace members of a checkout with
`skif_workspace::declared_member_dirs`: the directories with a `package.json` that the root's patterns select
(`pnpm-workspace.yaml` or the `workspaces` of `package.json`, negations applied), which are the patterns knip reads.
The root is a workspace too. A project directory is one whose `package.json` has a field besides `type`,
`sideEffects` and `imports`, as skif's file discovery decides it; the project of a path is the deepest such directory
above it. `parity.json` records the number of members per repository as `workspace_members`.

**`knip-unjudged-manifest`.** A finding only skif reports in `dependencies`, `devDependencies`,
`optionalPeerDependencies` or `binaries`, keyed on a `package.json` that is not one of knip's workspaces and that knip
reports no finding on.

**`non-member-project`.** A finding only knip reports whose project is not one of knip's workspaces: an unused file,
or an unlisted dependency that the project's own manifest lists in `dependencies`, `devDependencies`,
`peerDependencies` or `optionalDependencies`. Other issue types there stay compared, and so does an unlisted
dependency the project does not list.

**`skipped-directory`.** A finding only knip reports, in any issue type, in a file below a directory skif's file
discovery skips by name unless the directory is a workspace member: the `SKIPPED_DIRS` of
`crates/skif-js/src/discover.rs` (`out`, `target` and `coverage` only directly in the root or a package directory)
and every hidden directory but its `HIDDEN_DIRS`. The harness keeps a copy of those lists.

**`published-source`.** An unused file only knip reports that is a TypeScript source (`.ts`, `.tsx`, `.mts`, `.cts`,
not a declaration file) whose path, relative to the package that holds it, is published with a built extension: a
`main`, `module`, `source`, `types`, `typings`, `browser`, `bin` or `exports` leaf (not the catch-all `./*`) that
names the same path with `.js`, `.mjs`, `.cjs`, `.jsx` or a `.d.ts` variant, a `*` matching one or more characters
across `/`. The mapping from a build directory to `src` (`out/main.js` to `src/main.ts`) is not checked, so those
files stay compared.

**`fixture-file`.** A finding only knip reports below a `fixtures` or `__fixtures__` directory in `files`, `exports`,
`types`, `nsExports`, `nsTypes`, `enumMembers`, `namespaceMembers`, `unresolved` or `unlisted`: the issue types skif
does not report there. `--include-test-files` still counts the `files` part as differences.

**Precedence.** `knip-unread`, the file classifications (`non-code-file`, `fixture-file`, `test-file`), the wider
`fixture-file`, `not-judged-by-skif`, then `skipped-directory`, `non-member-project`, `published-source` and
`knip-config-error`. Every category is counted per repository in `parity.md`, recorded in the baseline under
`known_deviations` and ignored by `--check`.

**Not set apart.** Three clusters of the adjudication stay compared because the harness cannot check them from its
data: unlisted dependencies that a `declare module` satisfies (skif's report does not say which ambient declarations
it read, ADR 0049), npm's `lib/commands` files that skif reaches through ``require(`./commands/${name}`)`` in
`lib/npm.js` and knip does not (a difference in reading, not a path knip cannot read, so `knip-unread` does not fit),
and published paths mapped to source through a build directory.

### Consequences

* Good, because the one-sided counts of the recursive repositories (next-js, vercel-examples, mastra, vscode) and of
  angular and rules-js now mostly hold findings that need a judgement.
* Good, because every category is decided from manifests and paths, so the same rule applies to a repository added
  to the corpus later.
* Bad, because the committed parity baseline must be refreshed: `--check` against one recorded before this change
  reports the moved counts as drift.
* Bad, because `non-member-project` and `published-source` check where a finding is, not why skif has no finding
  there; a file skif's nested root wrongly counts as used is set apart too.
* Bad, because `skipped-directory` copies skif's directory lists, which can drift from `discover.rs`.
* Bad, because knip's workspaces are read with skif's pattern semantics; a pattern the two read differently moves a
  manifest between compared and set apart.

### Confirmation

* `skif-bench` unit tests: `fixture_findings_cover_every_category_skif_is_silent_in_there`,
  `findings_only_skif_reports_on_manifests_knip_does_not_judge_split_off`,
  `knip_only_files_and_listed_unlisted_below_non_member_projects_split_off`,
  `knip_only_findings_below_directories_skif_discovery_skips_split_off`,
  `typescript_sources_of_published_paths_split_off`, `a_star_matches_one_or_more_characters_across_directories`,
  `the_harness_reads_members_and_manifests_from_the_checkout` and
  `earlier_deviations_take_precedence_over_the_layout_ones`.
* The parity baseline in `tools/bench/baselines/parity.json` is recorded with these categories.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Four new path and manifest categories and a wider `fixture-file`

* Good, because each category states one checkable condition and the side and issue types it covers.
* Bad, because the known-deviations table of `parity.md` grows to ten columns.

### One `nested-root` category for every finding below a non-member project, on both sides and in every issue type

* Good, because it is one rule.
* Bad, because it would also hide exports, unresolved imports and unlisted dependencies that one tool reports in a
  nested project for reasons the location does not explain.

### Leave the counts as they are and explain the clusters in the documentation

* Good, because the baseline would not move.
* Bad, because a gate cannot tell several thousand findings about where each tool looks from real differences.

## More Information

* Amends: [ADR 0068](0068-what-the-knip-comparison-sets-apart.md) (`fixture-file` covers every issue type skif does
  not report in a fixture directory)
* Related decisions: [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md),
  [ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md),
  [ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md),
  [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md),
  [ADR 0049](0049-js-corrections-from-adjudicating-knip-disagreements.md),
  [ADR 0050](0050-fixture-directories-in-the-js-analysis.md)
* See the [ADR index](index.md).
