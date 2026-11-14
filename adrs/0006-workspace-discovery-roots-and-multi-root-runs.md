---
status: accepted
superseded-in-part-by: "0034"
date: 2026-11-13
decision-makers: ["Na'aman Hirschfeld"]
---

# Workspace Discovery, Roots and Multi-Root Runs

## Context and Problem Statement

Every rule needs the same facts about a repository: which package manager it uses, which directories are packages,
which dependencies each manifest declares and where in the file, and which catalogs exist. npm, pnpm, Yarn, Bun, Rush
and Lerna declare those facts in different files with different pattern semantics, and repositories often hold a
broken member manifest, a stray lockfile or no root `package.json` at all.

Many repositories also hold several independent projects: workspaces nested in workspaces, standalone packages next to
them, fixture trees full of `package.json` files. `skif .` at the top must lint each project exactly once with its own
configuration, a failure in one must not hide the others, and code or dependencies one project uses from another must
not be reported unused by the project that owns them. How does skif discover workspaces and roots, and how does a
multi-root run combine them?

## Decision Drivers

* Discovery reports facts; rules judge them, so every judgement is configurable and suppressible
* One semantics for workspace patterns, whatever the package manager
* A broken member manifest is a finding, not an aborted run
* Spans for every dependency entry, captured once
* Each package is linted by exactly one root, and a project gets the same result however a run reaches it
* A multi-root run lints each root once and still credits uses across roots

## Considered Options

* Fact-only discovery with pnpm pattern semantics, nested workspaces as roots of their own, and crediting across roots
  after every root is linted
* Reproduce each package manager's pattern semantics exactly
* Lint a multi-root repository as one merged workspace
* Lint nested roots twice, once alone and once inside the enclosing root

## Decision Outcome

Chosen option: "Fact-only discovery with pnpm pattern semantics, nested workspaces as roots of their own, and crediting
across roots after every root is linted", because it keeps judgement in rules, gives every rule and fix spans captured
once, and makes a recursive run both complete and free of duplicate findings without a second analysis.

### Facts, not judgements

`skif-workspace` returns facts only and never decides that a fact is a problem; rules do
([ADR 0008](0008-rule-and-diagnostic-model-rule-ids-and-severities.md)). All reads go through `FileSystem`
([ADR 0003](0003-library-api-and-embedding.md)).

### Package-manager detection

The first match wins:

1. `[workspace] package-manager` in the configuration;
2. the root manifest's `packageManager` field, then `devEngines.packageManager` (an object or a list, first known
   entry);
3. the manager a workspace declaration names: `rush.json` (`pnpmVersion`, `npmVersion`, `yarnVersion`), then
   `lerna.json` (`npmClient`);
4. a lockfile: `pnpm-lock.yaml`, then `bun.lock` or `bun.lockb`, then `yarn.lock`, then `package-lock.json`;
5. a `pnpm-workspace.yaml`;
6. the files Yarn Berry leaves (`.yarnrc.yml`, `.yarn/releases`);
7. a root `bunfig.toml` with an `[install]` table;
8. otherwise none.

skif never guesses npm. With no manager, every fix whose correctness depends on the manager (the `workspace:` protocol,
where a catalog lives, which install to run) is withheld. A declared field beats a lockfile because it is the project's
intent, and stray lockfiles are common in migrating repositories; rules compare the winner with the lockfiles on disk
([ADR 0011](0011-lockfile-and-toolchain-rules.md)). The header of `yarn.lock` tells Yarn Classic from Berry when the
field does not.

### Workspace declarations

* A directory declares a workspace with a `workspaces` key in `package.json`, a `pnpm-workspace.yaml`, a `rush.json`
  (`projects[].projectFolder`, matched as literal paths), or a `lerna.json` `packages` list, which counts only when
  nothing else declares members ([ADR 0032](0032-nx-and-lerna-and-pruned-output-trees.md)).
* The declarations rank `workspaces`, then `pnpm-workspace.yaml`, then `rush.json`, then `lerna.json`. Under pnpm a
  `pnpm-workspace.yaml` decides, and one without `packages:` declares pnpm's implicit `.` and `**`.
* A pnpm-workspace file that holds only settings, whose implicit patterns find no package besides the root, is no
  workspace: the root is a single package.
* A root `package.json` is optional. Without one the workspace is discovered with an empty stand-in for the root that
  no rule reports on and no summary counts, so the rules that judge the root manifest skip it.
* A root `package.json` with a BOM, comments or trailing commas is tolerated.

### Pattern semantics

* Patterns use pnpm semantics for every manager: `*` matches direct children, `**` descends (through packages, so a
  package inside a package is still a member), a segment may mix literal text and wildcards, and brace alternation is
  supported.
* Negations apply after all positive patterns, regardless of order.
* Discovery never enters `node_modules` or dot directories, never follows a pattern out of the root, and stops at a
  pattern depth of 64.
* Paths are root-relative strings with `/` separators, so results do not depend on how the root was spelled or on the
  platform.
* Bun's exceptions (a literal member survives a negation, members outside the root, nameless members) are in
  [ADR 0012](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md).

### The `Workspace` value

* Packages addressed by `PackageId` (the index in the sorted list), so two packages with the same or no name stay
  distinct.
* One CST-aware parse per manifest with a span table, so every dependency entry, field and pattern has its span without
  a second parse.
* One `DependencyInstance` per package, dependency kind, name and specifier, each holding a specifier parsed once during
  discovery ([ADR 0007](0007-specifier-and-semver-model.md)).
* The catalogs of the active package manager's files, with their spans.
* The matches of every pattern, including patterns that matched nothing and directories without a `package.json`.
* `manifest_errors`: members whose manifest cannot be read or parsed, as typed data with path, line and column, reported
  by `invalid-workspace-file` ([ADR 0008](0008-rule-and-diagnostic-model-rule-ids-and-severities.md)). The rest of the
  workspace is still discovered.
* Manifests are read and parsed in parallel, and the output is sorted, so it does not depend on scheduling.
* A workspace claims every directory its patterns select that holds a `package.json`, even one that does not parse.

### Single-package mode

When nothing declares a workspace, the root package is the only package (`WorkspaceSource::SinglePackage`). A
standalone package is published and keeps its dependencies in its own manifest, so `root-package-private-field` and
`root-package-dependencies` are silent in this mode. Every other rule runs unchanged. A declared list that matches no
member keeps its declared source.

### Roots

* `find_workspace_root` takes an absolute path and returns the nearest ancestor, the start included, that declares a
  workspace, or the project there when no ancestor's workspace lists it. It never climbs above the git repository root,
  so a manifest above the repository is never the workspace.
* A directory that declares a workspace is always its own root, also when an outer workspace lists it or it lies below a
  member. The outer workspace leaves its directory to it, so each package is linted by exactly one root.

### Multi-root runs

* `-r`/`--recursive`, or a start path with no package or workspace at or above it, selects a multi-root run. The napi
  addon's `recursive` option does the same.
* The walk below the start skips `node_modules`, hidden directories, symbolic links, directories excluded by a
  `.gitignore` or `.ignore`, and `[workspace] discover-ignore`. It stops at a depth of 32, and says so when the limit
  cut it. A directory it cannot list is recorded as unreadable, and its siblings are still searched. A directory skipped
  by name is entered only for the workspace members below it, and then only for them.
* `discover-ignore` defaults to `__fixtures__`, `fixtures`, `__mocks__` and `__snapshots__`. It is gitignore syntax
  relative to the start directory, replaces the default rather than extending it, is validated per entry, and is read
  from the configuration that governs the start directory, because discovery runs before any root has a configuration.
  It concerns root locations only; what the JS analysis reads in fixture directories is decided in
  [ADR 0014](0014-files-the-js-analysis-reads-walk-git-index-fixtures.md).
* The `json/` and `full/` trees of pruned output are skipped ([ADR 0032](0032-nx-and-lerna-and-pruned-output-trees.md)).
* Discovery is `skif::discover_targets`, built on `scan_candidates` and `claim_roots`; there is no second root-discovery
  entry point.
* Each root takes its layered configuration ([ADR 0004](0004-configuration-model-and-layering.md)); roots with equal
  configuration share one `Linter`.
* A root that cannot be loaded (an invalid manifest, a configuration error) is reported in the run's `errors` with exit
  code 2, and the other roots are linted. A workspace that cannot be loaded still claims its declared directories.
* The exit code is the worst over all roots. `--max-warnings` bounds the warnings of the whole run; without it each root
  applies its own `fail-on-warnings`.
* Roots are linted in parallel and results keep path order. `--fix` plans, writes and reports per root.
* In a multi-root run, a standalone package with another root above it is nested (`Workspace::is_nested`), and
  `root-package-manager-field` does not report for it; the outer project decides the manager.
* The report of a multi-root run is described in [ADR 0022](0022-reporting-and-output-formats.md).

### Crediting across roots

One lint per root, and the uses one root sees in another root's territory are credited after every root is linted:

* A dependency listed only above a root is recorded with the directory of the innermost manifest that lists it, where
  Node.js finds it (`Report::ancestor_dependency_uses`). The innermost run root that holds that manifest drops its
  `unused-dependencies` and `unused-dev-dependencies` findings for the name.
* A file another root reaches is listed in `Report::other_project_files`: a nested root's file that this root runs or
  imports, a file a plugin names there, and every module this root's graph reached outside its own directory. A file
  below the listing root goes to the nested root that owns it as an extra entry; a file outside it is credited, and the
  owning root drops its `unused-files` finding.
* A file outside the root is followed but not judged (`Module::foreign`); files below `node_modules` are installed code
  and are left out. The packages such a file imports are credited to the manifest at or above that file that lists them.
* A use of a manifest or file outside every root of the run is dropped. The lint after `--fix` credits the same uses, so
  the re-lint judges each root as the first lint did. A root linted alone still reports these findings.

### Consequences

* Good, because every judgement is an ordinary rule with a configurable severity.
* Good, because a broken member manifest still yields a report of everything else.
* Good, because `skif .` at the top of a repository of independent projects lints each project once, with its own
  configuration, and a failure in one does not hide the others.
* Good, because dependencies and files another root uses are not reported unused, and `--fix` does not remove them.
* Bad, because pnpm's pattern semantics differ from npm and Yarn at rare edges, which skif does not reproduce.
* Bad, because a host that lints roots itself must apply the crediting, or it reports those findings.
* Bad, because a root linted alone cannot see the uses of the roots around it.

### Confirmation

* `crates/skif-workspace/tests/memory.rs`: `package_manager_precedence`,
  `yarn_berry_files_identify_yarn_when_nothing_stronger_does`, `glob_semantics`, `negations_use_whole_paths`,
  `patterns_cannot_climb_out_of_the_root`, `invalid_member_json_is_collected_not_fatal`,
  `package_ids_are_dense_and_follow_directory_order_even_with_broken_members`,
  `rush_json_declares_the_members_and_the_package_manager`,
  `lerna_json_is_not_read_when_something_else_declares_the_members`,
  `a_pnpm_workspace_file_alone_is_a_workspace_without_a_root_manifest` and
  `implicit_patterns_that_select_no_member_make_a_single_package`.
* `crates/skif-workspace/tests/discovery.rs`: `a_package_without_a_workspace_is_a_single_package` and
  `double_star_does_not_descend_symbolic_links`.
* `crates/skif/tests/root.rs`: `a_project_with_its_own_workspace_is_its_own_root`,
  `a_manifest_above_the_repository_is_never_the_workspace` and `relative_start_paths_are_rejected`.
* `crates/skif-cli/tests/multi.rs`: `a_nested_workspace_is_a_root_and_every_package_is_linted_once`,
  `a_root_that_cannot_be_loaded_does_not_stop_the_others`, `the_exit_code_is_the_worst_across_roots`,
  `fixture_directories_are_not_searched_for_roots_unless_configured`, `a_walk_cut_by_the_depth_limit_says_so`,
  `the_package_manager_field_is_asked_of_workspaces_and_the_outermost_root_only`,
  `a_dependency_only_a_nested_root_uses_is_used_by_the_outer_manifest`,
  `a_file_a_sibling_root_imports_and_its_dependencies_are_used` and
  `the_lint_after_fixing_judges_each_root_as_the_first_lint_did`.

## Pros and Cons of the Options

### Fact-only discovery with pnpm pattern semantics, nested workspaces as roots of their own, and crediting across roots after every root is linted

* Good, because one pattern implementation serves every manager and every rule.
* Good, because crediting reuses the reports and needs no second analysis of the files a root owns.
* Bad, because each report carries the uses it saw elsewhere, which a single-root run does not need.

### Reproduce each package manager's pattern semantics exactly

* Good, because each manager's edge cases would match.
* Bad, because it multiplies the implementation for differences that do not matter to linting, and the managers accept
  the same pattern shapes.

### Lint a multi-root repository as one merged workspace

* Good, because one graph sees every use.
* Bad, because independent projects have their own package managers, catalogs and policies, which a merge mixes up.

### Lint nested roots twice, once alone and once inside the enclosing root

* Good, because each lint sees its neighbours.
* Bad, because every finding in a nested root appears twice and the work doubles.
