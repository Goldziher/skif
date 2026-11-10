---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
---

# Install Hooks Are Not Project Source, SvelteKit Route Types Before Sync, and Builtin-Named Packages

## Context and Problem Statement

Adjudicating the remaining one-sided unused dependencies and unresolved imports of the knip comparison (GitHub
issues #98 and #99) left three constructs where skif's answer was decided by accident rather than by a rule:

* A project whose manifest only lists what code elsewhere installs it for, with no source file, no entry and no
  script, is not judged for unused dependencies: its entries are unknown. vscode's `remote` has such a manifest, but
  also a `.pnpmfile.mjs`, the hook pnpm loads while it installs the project. The hook counted as a source file, so
  all 51 dependencies of `remote` were reported unused.
* A SvelteKit route imports its generated types as `./$types`. Its `tsconfig.json` (or `jsconfig.json`) extends
  `./.svelte-kit/tsconfig.json`, which `svelte-kit sync` writes with `rootDirs` that put `.svelte-kit/types` beside
  the sources. In a fresh checkout that config and the types are absent, so every `./$types` import was reported
  unresolved. The `rootDirs` case of a config that exists (React Router's `./+types/root`) is already handled.
* A bare import of a builtin name (`import { EventEmitter } from 'events'`) counts as a use of a listed npm package of
  that name (the `events`, `buffer` and `util` polyfills). knip does not count it, so it reports the package unused.

## Decision Drivers

* A manifest skif cannot see the users of must not be judged by a file that is not project code
* A generated import must not be reported while the generator has not run, and a typo must still be reported once it
  has
* Removing a dependency a bundle needs is the most costly false finding

## Considered Options

* Settle each construct at the place that decides it
* Keep the current behaviour and let users add `ignore-dependencies` and `ignore-unresolved` entries

## Decision Outcome

Chosen option: "Settle each construct at the place that decides it", because each has a rule that holds beyond the
corpus.

**Install hooks.** `.pnpmfile.cjs` and `.pnpmfile.mjs` are install hooks: the pnpm plugin still makes them entries, so
what they import is used, but they are not source. A package whose only files at or below its directory, and whose
only entries, are install hooks, and that has no script, has unknown entries as a package with no source file does.

**SvelteKit route types.** A relative import `./$types`, `./$types.js` or `./$types.d.ts` in a script is not
unresolved while a relative `extends` entry of the nearest `tsconfig.json` or `jsconfig.json` names a file that does
not exist and that git ignores. Once the extended config exists, its `rootDirs` resolve the import, and a route type
that is still missing is unresolved like any other file.

**Builtin-named packages.** A bare builtin specifier (`events`, `buffer`, `util`) is a use of a listed package of the
same name: webpack 5, Vite and other bundlers resolve it to the installed package when they build for the browser,
which is why projects list these polyfills. A `node:` specifier names the builtin only and uses no package; a
trailing slash (`events/`) names the package, as before
([ADR 0044](0044-monorepo-js-corrections-builtins-html-entries-referenced-paths-extglob-helpers-and-stylesheets.md)).
skif and knip disagree on these packages by design.

### Consequences

* Good, because vscode's `remote` manifest is reported among `incomplete-packages` instead of with 51 false unused
  dependencies.
* Good, because SvelteKit projects in a fresh checkout get no unresolved imports for their route types.
* Bad, because a polyfill listed but used only by Node.js code, where the builtin always wins, is not reported unused.
* Bad, because only pnpm's hook files are known as install hooks; another package manager's hook written in
  JavaScript still counts as source.

### Confirmation

* `crates/skif-js/tests/monorepo_regressions.rs`: `a_pnpmfile_alone_does_not_give_a_project_source_whose_dependencies_are_judged`,
  `a_pnpmfile_uses_what_it_imports_in_a_project_with_source` and
  `bare_builtin_imports_use_a_listed_package_of_that_name`.
* `crates/skif-js/src/resolve.rs`: `sveltekit_route_types_are_unknown_while_the_extended_config_is_not_generated`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Settle each construct at the place that decides it

* Good, because the findings are right without configuration.
* Bad, because the route type rule is tied to SvelteKit's `$types` name.

### Keep the current behaviour and let users add `ignore-dependencies` and `ignore-unresolved` entries

* Good, because nothing changes.
* Bad, because every SvelteKit project and every project with only a pnpm hook needs the same entries.

## More Information

* Related decisions: [ADR 0044](0044-monorepo-js-corrections-builtins-html-entries-referenced-paths-extglob-helpers-and-stylesheets.md),
  [ADR 0053](0053-incomplete-packages-in-the-json-report.md),
  [ADR 0068](0068-what-the-knip-comparison-sets-apart.md)
* See the [ADR index](index.md).
