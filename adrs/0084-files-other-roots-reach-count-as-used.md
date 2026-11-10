---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
---

# Files Other Roots Reach Count as Used, Members Below Hidden Directories Are Analysed, and Jest's testRegex Adds Entries

## Context and Problem Statement

A recursive run lints each root on its own. Two channels already carry what one root's code does with another
root: the files of a nested root that an enclosing root imports become entries of the nested root's run
([ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md)), and the dependencies a nested
root uses through the enclosing manifest are credited to that manifest
([ADR 0077](0077-dependencies-nested-roots-use-count-for-the-enclosing-manifest.md)). Three uses of files have no
channel, so the owning root reports a used file as unused, and `--allow-remove-files` deletes it.

* A file of a nested root that a tool configuration of the enclosing root names. On the pinned corpus, vscode's
  workflows run `node build/azure-pipelines/common/extract-telemetry.ts` and `node build/lib/screenshotDiffReport.ts`;
  `build` is a nested root and its run never learns of the workflow. Only imports were passed inward, and the
  plugin entries of the enclosing root match only its own files.
* A file of the enclosing root that a nested root imports: vscode's `build/lib/test/detectChanges.test.ts` imports
  `../../../.github/actions/detect-changes/detect.ts`, a file of the root at the top.
* A file of a sibling root (two standalone projects side by side) that the other one imports.

The `jest` plugin does not read `testRegex`, so the test files a config selects with it, rather than with
`testMatch`, are reported unused.

Separately, the file discovery of the JS analysis skips hidden directories other than tool configuration ones and
build output by name, unless the directory is a workspace package. vscode lists `.vscode/extensions/*` as
workspace members; the packages load, but the walk never enters `.vscode`, so none of their files is analysed.

The importing root does not even see such files: its module resolver takes a file outside the workspace root for an
asset, so the graph neither parses it nor follows its imports, and the root that owns the file is never told of the
use.

## Decision Drivers

* A file another root of the run uses must not be reported unused, nor deleted by a fix
* Multi-root runs stay one lint per root ([ADR 0077](0077-dependencies-nested-roots-use-count-for-the-enclosing-manifest.md))
* A root linted on its own reports what it can see

## Considered Options

* Extend `Report::other_project_files` and credit the files outside the importing root after every root is linted
* Re-lint the owning root with the files as extra entries
* Analyse files outside the root as entries of a merged graph

## Decision Outcome

Chosen option: "Extend `Report::other_project_files` and credit the files outside the importing root after every
root is linted", because it reuses both existing channels and keeps one lint per root.

**Following files outside the root.** The resolver resolves an import of a source file outside the workspace root
to that file, unless it lies below a `node_modules` directory, and the graph takes it for a file of another project
(`Module::foreign`), as it does for a nested root's file: it is parsed and its imports are followed, but it is not
judged, and its imports use no dependency of the importing root nor are unlisted there.

**What a report lists.** `Report::other_project_files` lists every path absolute. It keeps the files below the
workspace root that belong to a project of their own, and now lists two more kinds:

* below the root, the files of another project that a script runs (already graph entries, never listed) and the
  files a plugin names as a single path (a workflow step, a config's `globalSetup`) that exist and are no file of
  this analysis;
* outside the root, with absolute paths: every module the graph reached there, through imports of this root's code
  or of other such modules, and the files a plugin names there. Files below a `node_modules` directory are installed
  code and are left out.

**Routing.** `skif -r` hands each listed file to the innermost root of the run that holds it, other than the
listing root. A file below the listing root goes to a root nested in it, which is linted later, as an extra entry,
as before. A file outside the listing root is credited once every root is linted: the owning root drops its
`unused-files` diagnostic for the file with `Report::retain`, as ADR 0077 does for dependencies. The lint after
`--fix` credits the same files. A file outside every root of the run is dropped.

**Dependencies of credited files.** The packages a file outside the listing root imports are resolved by Node.js
from that file's directory upwards, so they are used dependencies of the manifest there, not of the listing root.
`Report::ancestor_dependency_uses` lists each of them with the innermost manifest at or above the file, up to the
repository root, that lists it, and `skif -r` credits it to the root that owns that manifest exactly as it credits the
dependencies a nested root uses (ADR 0077): a sibling's `lib.js` that requires `lodash` keeps `lodash` used in the
sibling's `package.json`, and `--fix` does not remove it.

**Members below a skipped directory.** The walk enters a directory it skips by name when a workspace package lies
below it, and then only for those packages: the files of that directory and of its other subdirectories stay out,
and hidden directories inside the members are skipped as before. The `skipped-directory` deviation of the knip
comparison ([ADR 0080](0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md))
follows: a file in a member below a skipped directory is compared.

**Jest's `testRegex`.** The `jest` plugin reads `testRegex` (a string, a regular expression literal or an array of
them) as regular expressions that select test files (`PluginInput::EntryRegex`, `PluginOutput::entry_regexes`,
`WorkspaceInputs::entry_regex`). They add entries and never replace the default test globs, although Jest runs
`testRegex` instead of `testMatch`: knip 6.44.1's `jest` plugin ignores `testRegex` and keeps its defaults, so a
replacement would report files knip counts as tests, and `--allow-remove-files` would delete them. A regular expression
is matched against `/` followed by the path relative to the package directory. `<rootDir>` stands for Jest's `rootDir`
resolved against the config's directory (the config's own directory by default, the enclosing config's for a project
object), and a leading one anchors the expression there; a `rootDir` that is computed, absolute or outside the package
makes `<rootDir>` match any directory, so no file the expression selects is missed. The `i`, `m` and `s` flags of a
literal become inline flags. An expression the `regex` crate does not compile (a lookaround, a backreference, one over
its size limit) leaves the entries incomplete instead of being guessed at. Each expression is compiled once by the
plugin to check it and once more per package, together with the others, to match.

`testPathIgnorePatterns` is not applied. knip ignores it as well, so applying it would report test files knip counts
as used, with the same risk of deletion; and since it narrows only Jest's own test files, applying it would also mean
keeping each tool's entries apart from the others'.

The files outside a root are credited, not made entries, because the owning root may already be linted (it encloses
the listing root or sits beside it), and because the listing root's graph already followed everything they import:
crediting the whole set needs no second lint.

### Consequences

* Good, because vscode's files that only the outer workflows run or only the `build` tests import are no longer
  reported, and `--allow-remove-files` keeps them.
* Good, because a library host gets the same data `skif -r` uses and decides how to apply it.
* Bad, because a credited file's exports are not judged against the code that uses it: the owning root never
  analysed that file as reached, and the importing root does not judge files it does not own. Unused exports of
  such a file go unreported.
* Bad, because a root linted on its own still reports these files, as for the other channels.
* Neutral, because a plugin entry is listed only when it names one path; a glob that names files of another root is
  still not followed there.
* Bad, because a test file that only Jest's `testPathIgnorePatterns` excludes, or that only the default test globs
  match in a config whose `testRegex` replaces them for Jest, stays an entry and is not reported, as knip does not
  report it.
* Bad, because a `testRegex` the `regex` crate does not compile leaves the package's files unjudged.

### Confirmation

`crates/skif-cli/tests/multi.rs` checks that `skif -r` keeps a file of a nested root that a workflow of the outer
root runs, a file of the outer root that the nested root requires together with the file that one requires, and a
file a sibling root requires together with the dependency that file imports; that each root linted on its own still
reports them; and that `--fix-unsafe --allow-remove-files` keeps them and deletes the unused files and dependencies
beside them. `crates/skif-js/tests/fp_resolve.rs` checks that a file outside the root is followed, listed with an
absolute path and judged by no rule of the importing root, and that its dependency is listed with the sibling's
manifest. `crates/skif-js/src/discover.rs` checks
that the files of `.vscode/extensions/*` members are found and owned by them while other files of `.vscode` are not,
and `tools/bench/src/parity.rs` that the knip comparison sets apart a file below `.vscode` only outside a member.
`crates/skif-js-plugins/tests/plugins.rs` checks that `testRegex` keeps the default test globs, translates flags,
anchors `<rootDir>` at the `rootDir` of each config and project (any directory when that is unknown) and leaves the
entries incomplete for an expression the `regex` crate rejects; `crates/skif-js/tests/graph_fixes.rs` that the files it
selects are entries, that `testPathIgnorePatterns` removes none, and that an anchored expression matches only below the
root directory; `crates/skif-js/src/globs.rs` that `\/__tests__\/` compiles and matches.

## Pros and Cons of the Options

### Extend `Report::other_project_files` and credit the files outside the importing root after every root is linted

* Good, because each root is still linted once
* Bad, because the exports of a credited file are not judged

### Re-lint the owning root with the files as extra entries

* Good, because the owning root would judge the files and their exports
* Bad, because the root that encloses all others, usually the largest, would be linted twice

### Analyse files outside the root as entries of a merged graph

* Good, because one graph would see every use
* Bad, because files would be judged against the wrong manifest, which is what nested roots exist to avoid

## More Information

* Extends: [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md),
  [ADR 0077](0077-dependencies-nested-roots-use-count-for-the-enclosing-manifest.md)
* See the [ADR index](index.md).
