---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
---

# Dependencies Nested Roots Use Count for the Enclosing Manifest

## Context and Problem Statement

A nested root is a project below another root with a `package.json` of its own that is not a workspace member (a
`build` folder with its own lockfile). Its files belong to it alone: the enclosing root does not analyse them, and an
import there that the nested root does not list is resolved against the manifests above it, up to the repository
root, so it is not reported unlisted. Node.js resolves such an import from the enclosing project's `node_modules`, so
the enclosing manifest's entry is what makes it work. The enclosing root never sees that use and reports the entry as
unused, which is the most costly false positive skif has: the fix deletes a dependency the build needs.

On the pinned corpus, vscode's root `devDependencies` hold 26 such entries (`gulp-rename`, `gulp-filter`,
`vinyl-fs`, `debounce` and more), each imported only by `build/**/*.ts` of the nested root at `build`, whose own
manifest lists none of them (GitHub issue #98). Multi-root runs already pass information inward: the files of a nested
root that an enclosing root imports become entries of the nested root's run, so nested roots are linted after the
roots enclosing them. Information that flows outward has no channel yet.

## Decision Drivers

* An unused-dependency finding must not depend on code the run analysed under another root
* A root linted on its own reports what it can see, as for the files other roots import
* Multi-root runs stay one pass per root: re-linting the enclosing root is too slow on large trees

## Considered Options

* Report the uses and drop the enclosing root's findings after every root is linted
* Analyse the files of nested roots as part of the enclosing root
* Re-lint the enclosing root with the uses as an input

## Decision Outcome

Chosen option: "Report the uses and drop the enclosing root's findings after every root is linted", because it keeps
one lint per root and needs no second analysis of the files a nested root owns.

**Recording.** The dependency analysis of a root remembers, for every name listed only above the workspace root, the
directory of the innermost manifest that lists it, which is where Node.js finds it. An import, a plugin reference or a
script binary that resolves through such a manifest records the use. `Report::ancestor_dependency_uses` lists them as
the manifest's absolute directory and the dependency name, sorted; the field is not serialised, like
`Report::other_project_files`.

**Crediting.** Once every root of a `skif -r` run is linted, each use goes to the innermost root of the run that holds
the manifest's directory. That root drops its `unused-dependencies` and `unused-dev-dependencies` diagnostics for the
manifest and name with `Report::retain`, which keeps the order and counts the summary again. The lint after `--fix`
credits the same uses, so the re-lint judges the root as the first lint did. A use of a manifest outside every root of
the run is dropped.

### Consequences

* Good, because the 26 vscode root entries that only `build` uses are no longer reported, and a `--fix-unsafe` run
  no longer removes them.
* Good, because a library host gets the same data a multi-root host needs and decides how to apply it.
* Bad, because the enclosing root linted on its own still reports these dependencies; as with the files enclosing
  roots import, a run from the top with `-r` is needed.
* Neutral, because the `@types` twin of an import is credited as the import resolves it, but a peer dependency of a
  package the nested root uses is not: the enclosing root's peer rule never sees the use.

### Confirmation

`crates/skif-js/tests/fp_resolve.rs` checks that a nested project's uses name the innermost ancestor manifest, and
`crates/skif-cli/tests/multi.rs` that `skif -r` keeps the enclosing root's dependency used, that the root linted on its
own still reports it, and that `--fix-unsafe` keeps it.

## Pros and Cons of the Options

### Report the uses and drop the enclosing root's findings after every root is linted

* Good, because each root is still linted once
* Bad, because the enclosing root's peer rule (a peer of a used host is used) does not see the use

### Analyse the files of nested roots as part of the enclosing root

* Good, because the enclosing root alone would know the uses
* Bad, because the files would be analysed twice and judged against two manifests

### Re-lint the enclosing root with the uses as an input

* Good, because every rule of the enclosing root would see the uses
* Bad, because the largest roots, the ones nested roots sit in, would be linted twice
