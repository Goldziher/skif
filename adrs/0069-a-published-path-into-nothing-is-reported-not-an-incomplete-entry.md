---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0065"]
---

# A Published Path Into Nothing Is Reported, Not an Incomplete Entry

## Context and Problem Statement

[ADR 0065](0065-narrowed-scope-for-incomplete-entries.md) keeps a package unjudged as a whole when a published entry
maps to no source file (`unmapped-published`), since publishing may expose any file. A published path counts as
needing a source when it lies in a build output directory (`dist`, `lib`, `build`, ...) or in `bin`. The
`package-entry` hint of `stale-js-config` skips every such path, because build output is missing until the package is
built.

A `bin` path can also name a file that does not exist, with no source behind it and nothing that builds it:

```json
{ "name": "r2b", "private": true, "bin": { "r2b": "bin/r2b.js" },
  "scripts": { "start": "node app/main.js" },
  "dependencies": { "left-pad": "1.3.0" } }
```

with `app/main.js` and no `bin` directory. skif reports nothing about the path, lists the package in
`incomplete-packages` with `unmapped-published`, and does not report the unused `left-pad` (GitHub issue #109). The
path loads no code, so it cannot make anything used, yet it hides every finding of the package. How should a published
path that names nothing count?

## Decision Drivers

* A finding that depends on something the analysis could not read is a false positive
* A path into nothing loads no code, so it should not hide the findings of its package
* A path into nothing is itself a mistake worth reporting
* Build output that is simply not built yet must stay unjudged, as before

## Considered Options

* A path into nothing is no entry and is reported by `package-entry`
* A new rule for published paths into nothing
* Keep the package unjudged

## Decision Outcome

Chosen option: "A path into nothing is no entry and is reported by `package-entry`", because the existing hint already
reports published paths that name no file with the same judgement of what is build output, so one predicate decides
both what is reported and what stops making the package incomplete.

**A path into nothing.** A published path names nothing when it lies in the package, is not in a build output
directory (its first segment is not `dist`, `build`, `lib`, `out`, `esm`, `cjs`, `types` or a flavour of them), git
does not ignore it (unless `no-gitignore` is set), it is neither a file nor a directory, and no source file maps to it
(by the package's `tsconfig` build directories or the `dist` to `src` convention); a path with `*` names nothing when
neither it nor its source patterns match a file. This is the existing `package-entry` test, which now also applies to
paths in a `bin` directory, which should have a source behind them, in any published field: `bin/r2b.js`, an
extensionless `bin/tool`, or `main` naming `bin/cli.js`. A root `bin` such as `"bin": "cli.js"` was reported before.

**Ignored by git.** The ignore files count from the root of the git work tree the analysed root lies in, not only
from the analysed root as the file walk reads them, together with the repository's `info/exclude` (in the common
directory of a linked work tree). Outside a work tree only the ignore files from the analysed root down count. All
of them are read through the file system layer.

**A `bin` a build step may write.** A path in `bin` is build output not built yet, so it names nothing only when the
package has no build step that may write it. A build step is a `build`, `build:*`, `prepack` or `prepublishOnly`
script; a script that runs a compiler or bundler (`tsc`, `tsgo`, `tsup`, `tsdown`, `esbuild`, `rollup`, `rolldown`,
`webpack`, `rspack`, `rslib`, `unbuild`, `ncc`, `babel`, `swc`); a bundler configuration an enabled plugin reads
(`tsup.config.*`, `tsdown.config.*`, `esbuild.config.*`, `rollup.config.*`, `rolldown.config.*`, `webpack.config.*`,
`rspack.config.*`, `rslib*.config.*`, `build.config.*` of unbuild); or a `tsconfig` build directory (`outDir`) at or
inside `bin`. `prepare` alone is no build step, since it mostly installs git hooks; a `prepare` that builds runs a
compiler or the `build` script. Paths outside `bin` and outside a build output directory are not written by a build
the analysis can tell, so they keep the existing test.

**No entry, no incomplete package.** Such a path contributes no entry file and does not mark the package's entries
incomplete. The package is judged from its other entries (scripts, plugins, the other published paths), so its unused
files, dependencies, exports, types and members are reported. A package left with no entry, no script and no source
file is still unjudged, as before, since its manifest only lists what code elsewhere installs it for.

**Reported by `package-entry`.** `stale-js-config` reports it like any other published path that names no file:
"`bin/r2b.js` in `bin` of `package.json` names no file", pointing at the field in `package.json`. No rule ID is added.

**Unchanged.** A path in a build output directory that maps to no source stays `unmapped-published`, whether the
directory exists (built output whose source cannot be found) or not (a checkout that is not built, whose bundler may
name its output after no source file). A bin stub that exists, but loads output with no source behind it, keeps the
package unjudged too, and so does a gitignored path or a `bin` path a build step may write, which are build output
not built yet.

### Consequences

* Good, because a package with a stale `bin` path is judged, and the stale path is reported.
* Good, because the hint and the entry discovery share one predicate, so a path `package-entry` reads is either
  reported and no entry, or silent and keeps the package unjudged.
* Good, because a `bin` written by a build of the package, or hidden by any ignore file of the repository, keeps the
  package unjudged as before, so the source it is built from is not reported unused.
* Bad, because a `bin` file a build outside the package writes (a script of the workspace root or of another
  package), that no build step of the package names and that is hidden only by the global excludes file
  (`core.excludesFile`, which is not read) or by nothing, is reported until it is built, and the package is judged
  without it.
* Bad, because any build step keeps every `bin` path of its package unjudged, also one that is stale, as before.
* Bad, because a `source` field or an `imports` leaf, which `package-entry` does not read (knip does not), stops making
  the package incomplete when it names nothing in `bin`, without a finding: such a path is the one exception to a
  path being either reported or keeping the package unjudged.

### Confirmation

* `crates/skif-js/tests/config_hints.rs` checks the repro of the issue (the path reported, `left-pad` unused), a `bin`
  path in `exports` in the `published_paths_that_name_no_file` snapshot, a `bin` path hidden by a `.gitignore` of
  the root, by one above the analysed root and by `.git/info/exclude`, a `bin` path a build script, a script running
  `esbuild`, a `tsup` configuration or a `tsconfig` `outDir` may write, and that build output never is a path into
  nothing: a `dist` path whose source exists maps, an unbuilt or a built `dist` without a source and a bin stub
  loading output without a source keep the package unjudged.
* `crates/skif-js/tests/explain_absence.rs` checks that `explain-usage` judges a file of the repro package
  (`unused-file`, not `entries-incomplete`), and still gives `unmapped-published` for unbuilt `dist` output.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A path into nothing is no entry and is reported by `package-entry`

* Good, because the stale path is reported where knip reports it, with the message skif already uses.
* Good, because no rule ID is added.
* Bad, because a generated `bin` that no build step of its package names is called missing before a build.

### A new rule for published paths into nothing

* Good, because the finding could be configured apart from the configuration hints.
* Bad, because it splits one judgement, a published path that names no file, between two rules.

### Keep the package unjudged

* Good, because it never misjudges a generated `bin` file.
* Bad, because one stale path hides every finding of its package and is itself not reported.

## More Information

* Amends: [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md) (a published path into nothing is no longer an
  `unmapped-published` reason)
* Related decisions: [ADR 0034](0034-js-configuration-parity-with-knip.md),
  [ADR 0053](0053-incomplete-packages-in-the-json-report.md)
* See the [ADR index](index.md).
