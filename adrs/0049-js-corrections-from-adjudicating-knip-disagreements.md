---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0036"]
---

# JS Corrections from Adjudicating knip Disagreements: Linked Files, Subpath Imports, Aliased Packages, Computed Specifiers, Component Styles and Three Plugins

## Context and Problem Statement

The first published comparison with knip showed low agreement on unused exports and dependencies. Reading each cluster of disagreements against the repository sources showed that many were not disagreements: the same export under another label, or paths knip never read. The rest included false findings of skif. A file reached through a symbolic link was a separate module, so its exports were reported once per link and the file itself looked unused. The `imports` field was never read for entries, so a file only one condition selects looked unused. A package reached through a `tsconfig` path alias was not counted as used. `declare module 'pkg'` did not count either. A computed `require` was an unknown target, so all of npm's commands, loaded by ``require(`./commands/${name}`)``, looked unused to both tools, and everything only they import lost its use. ADR 0036 declined a `tap` plugin because knip has no row to follow, and npm's test suites were reported as unused as a result.

Which of these does skif change, and how does the comparison report the rest?

## Decision Drivers

* A false finding costs trust whether or not knip shares it
* The comparison must not count a labelling difference or a tool's blind spot as disagreement
* The analysis must not pay for symbolic links in trees that have none
* Declaring a module must not become a new way to report an unlisted import

## Considered Options

* Fix the false findings, add `tap` without a knip row, and teach the comparison about labels and unread paths
* Fix only the findings knip does not share, keeping ADR 0036's rule for `tap`
* Treat every symbolic link by canonicalizing each resolved path

## Decision Outcome

Chosen option: "Fix the false findings, add `tap` without a knip row, and teach the comparison about labels and unread paths", because agreement with knip is a measure and not the goal: both tools reporting a test file as unused is two tools being wrong.

**Linked files.** A symbolic link to a file the analysis also finds is that file: discovery removes it from the candidates and the graph interns it as its target. A link that leaves the workspace, or points at a file outside the analysis, stays a module of its own. Only links are canonicalized.

**Subpath imports.** Every literal leaf of `imports` is a manifest target, so each condition's file is reachable. A pattern leaf is not: its files are reached by the specifiers that use it.

**Aliased and augmented packages.** A specifier that an alias resolves to a file still counts as a use of the package of that name when the manifest lists it. So does `declare module 'pkg'`. Neither adds an import, so neither reports an unlisted dependency.

**Computed specifiers.** A template literal that starts with a relative path is a glob of the files it can name, with `*` for each expression, and each matching module counts as read in full. It stays narrow: ``import(`./pages/${name}.js`)`` matches one directory and one extension. Any other computed specifier stays an unknown target.

**Component styles.** A specifier in a style block of a `.vue`, `.svelte` or `.astro` file that the script resolver cannot find is looked up as a stylesheet, so a Sass partial resolves. It stays unresolved when neither finds it.

**Plugins.** `tap` is added with the default test locations tap documents, which amends the line of ADR 0036 that declined it: a plugin needs a documented behaviour to follow, which is not always a knip row. `oxfmt` follows knip, and so does `bun`, which adds an enabler for a text in a script: a package can use `bun test` without depending on anything that names bun. `jest` keeps a runner name that is already a jest package as written, and `eslint --format` loads `eslint-formatter-<name>`.

**Comparison.** `exports`, `types`, `nsExports` and `nsTypes` name the same `<file>|<export>`, so the same key under another label is agreement. skif's own labels do not change. `knip-unread` in `corpus.toml` lists the paths knip does not read in a repository, and one-sided findings below them are a known deviation.

### Consequences

* Good, because each correction removes false findings for every user, not only in the corpus.
* Good, because the published agreement says what the tools disagree on.
* Bad, because a dependency only reached through an alias is not reported even if the alias makes it redundant.
* Bad, because every literal `imports` target is an entry, so a mapping nothing imports keeps its file alive.
* Bad, because a computed specifier keeps every file it can name alive, including one no runtime value reaches.
* Neutral, because the plugin table holds 105 plugins.

### Confirmation

* `crates/skif-js/tests/fp_graph.rs` covers linked files, every condition of a subpath import, patterns not being entries, aliased packages, augmentation, computed specifiers and component styles.
* `crates/skif-js-plugins/tests/plugins.rs` covers `tap`, `oxfmt`, `bun` and the jest runner, and `scripts/tests.rs` covers the formatter names.
* `the_same_export_under_another_label_is_agreement` in `tools/bench/src/parity.rs` covers the labels.

## Pros and Cons of the Options

### Fix the false findings, add `tap` without a knip row, and teach the comparison about labels and unread paths

* Good, because it corrects skif and the measure separately.
* Bad, because `tap` has no upstream row to compare with.

### Fix only the findings knip does not share, keeping ADR 0036's rule for `tap`

* Good, because agreement alone would rise and every plugin keeps a reference.
* Bad, because the findings both tools share stay false.

### Treat every symbolic link by canonicalizing each resolved path

* Good, because it needs no list of links.
* Bad, because it adds a system call for every resolved import in every tree.
