---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0080", "0096", "0099"]
---

# The knip Comparison: n8n Node Files and the Files Published Sources Import

## Context and Problem Statement

Once n8n's `build-command` built the workspace's config packages, knip loaded the configurations it had failed on
([ADR 0099](0099-knip-comparison-narrowed-scopes-rush-installs-and-dependencies-of-unused-files.md)), and its
knip-only unused files rose from 7 to 1,867. About 1,850 of them lie in `packages/nodes-base/nodes` and
`packages/nodes-base/credentials`. That package lists its node and credential files in `package.json` `n8n.nodes` and
`n8n.credentials` as built paths (`dist/nodes/A/A.node.js`), and n8n loads them by path. skif makes those files
entries; knip does not read the fields, so it reports them and every file they import as unused.

The `published-source` deviation
([ADR 0080](0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md),
[ADR 0099](0099-knip-comparison-narrowed-scopes-rush-installs-and-dependencies-of-unused-files.md)) sets apart a
knip-only unused file whose path its package publishes, but only for `main`, `module`, `source`, `types`, `typings`,
`browser`, `bin` and `exports`, and it leaves the files only such an entry reaches compared. The listed node and
credential files account for about 600 of the 1,850; the files they import account for the rest. Both are one
difference, knip having no entry there, counted once per file.

n8n's `knip-invalid` mark was to stay until a parity run confirmed that knip loads those configurations. The run
confirms it: only two unrelated `vite.config` files still fail to load, and they are counted as `knip-config-error`.

## Decision Drivers

* A difference is counted once, where it arises
* A known deviation names its cause and is decided from data the harness has
* The harness reads a manifest field the way skif reads it

## Considered Options

* Read the n8n fields in `published-source` and add the files a published source imports through files only knip
  reports unused
* Give knip entry files for the n8n fields in the harness's knip configuration
* Keep `published-source` as it is and leave n8n marked invalid

## Decision Outcome

Chosen option: "Read the n8n fields in `published-source` and add the files a published source imports through files only knip reports unused", because it keeps knip's run unchanged, reads the fields with the code skif reads them with, and decides the reached files from the files and their imports, which the harness already parses.

**n8n node packages.** A package's published paths include the files of `n8n.nodes` and `n8n.credentials`, as
`skif_js::n8n_load_paths` reads them: each path as written and, below a build directory, also the path without that
directory, since an n8n node package compiles without a `rootDir` (`dist/nodes/A/A.node.js` names
`nodes/A/A.node.ts`). They name their source as the other paths without `*` do, the first of skif's source candidates
that exists.

**Files a published source imports.** A file knip reports unused and skif does not is `published-source` when a
published source among those files reaches it by relative imports, directly or through other such files. The harness
parses each file with `skif-js-syntax` and follows its relative specifiers (imports, type imports, requires,
re-exports and dynamic imports; not globs, test-runner mocks or specifiers with a query such as `?raw`), resolved as
the path itself, the path with a TypeScript or JavaScript extension (a `.js` one also replaced by its TypeScript
twin), or the `index` file of the directory it names. A file both tools report unused ends the chain. Bare and
aliased specifiers are not followed, and exports, types and members of these files stay compared.

**n8n is a valid comparison.** n8n's `knip-invalid` mark is dropped, so its row counts in the agreement sums.

### Consequences

* Good, because the n8n files knip cannot know are counted once, as the deviation they are, and the files left
  compared in that package are ones the harness cannot explain from paths.
* Good, because the harness and skif read the n8n fields through one function.
* Bad, because the reached files are decided from relative imports only; a file reached through a tsconfig `paths`
  alias stays compared.
* Bad, because the exports, types and members knip reports in the files the node files import stay compared, though
  they follow from the same missing entries.
* Bad, because the category set is unchanged but its counts move, so the n8n baseline entry has to be recorded again.

### Confirmation

* The harness unit tests in `tools/bench` treat the listed node and credential files of an n8n node package as
  published sources and an unlisted file of the package as not, collect the files a published source reaches through
  files only knip reports unused (stopping at a file both report unused, a `?raw` import and a specifier leaving the
  checkout), and resolve relative specifiers with extensions, TypeScript twins and `index` files.
* `no_repository_is_marked_as_an_invalid_knip_comparison` reads the corpus and finds no repository with
  `knip-invalid`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read the n8n fields in `published-source` and add the files a published source imports through files only knip reports unused

* Good, because knip's run stays the one every repository gets.
* Bad, because the downstream exports, types and members are not covered.

### Give knip entry files for the n8n fields in the harness's knip configuration

* Good, because knip would then judge the node files and everything below them, exports included.
* Bad, because the harness would teach knip skif's reading of a field, which no other repository gets, and the
  globs would have to be written per package.

### Keep `published-source` as it is and leave n8n marked invalid

* Good, because nothing more is set apart.
* Bad, because n8n's counts would mix about 1,850 files of one known cause with real differences, and the mark's
  reason no longer holds.

## More Information

* Amends: [ADR 0080](0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md) and
  [ADR 0099](0099-knip-comparison-narrowed-scopes-rush-installs-and-dependencies-of-unused-files.md)
  (`published-source` reads the n8n fields and covers the files a published source imports; n8n's mark is dropped)
  and [ADR 0096](0096-knip-comparison-findings-in-unused-files-invalid-runs-and-two-sided-config-errors.md) (no
  repository is marked invalid)
* Related decisions: [ADR 0068](0068-what-the-knip-comparison-sets-apart.md)
* See the [ADR index](index.md).
