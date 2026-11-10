---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0080", "0096"]
---

# The knip Comparison: Narrowed Incomplete Scopes, an Installed Rush Repository and Dependencies Only Unused Files Import

## Context and Problem Statement

[ADR 0080](0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md) and
[ADR 0096](0096-knip-comparison-findings-in-unused-files-invalid-runs-and-two-sided-config-errors.md) set apart the
one-sided findings that follow from where each tool looks. Re-reading the remaining one-sided findings showed five
gaps in what they cover, each a difference that says nothing about whether skif is right.

1. `not-judged-by-skif` sets apart every file, export, type and member of a package listed under
   `incomplete-packages`, although the JSON report now carries each entry's `scope` and skif judges the files outside
   a narrowed one ([ADR 0095](0095-untracked-files-member-manifest-errors-and-converging-fixes.md)). The count was an
   upper bound and hid findings skif did judge.
2. knip ran on rushstack uninstalled, in a scratch clone of the checkout without `node_modules`: it mapped no binary
   to a listed package (`heft`), credited no dependency through an installed manifest (`@rushstack/heft`), and could
   not load the rig configurations. The scratch root manifest also made the Rush root a workspace for knip, whose files
   skif does not analyse.
3. `non-member-project` covered unused files and listed unlisted dependencies only. The other findings in the files
   of such a project (unresolved imports, exports, members) have the same cause.
4. A dependency one tool reports unused because the only files importing it are files that tool lists as unused is
   downstream of the `files` disagreement, but `in-knip-unused-file` and `in-skif-unused-file` leave the manifest
   categories compared.
5. `published-source` did not follow a published path below a build directory to `src`, nor a published path without
   an extension, both of which skif maps to source.

Two more gaps were in how the harness prepares a checkout. n8n's tool configurations import the built `dist` of the
workspace's own config packages, which an install without lifecycle scripts never builds, so knip loaded none of them.
And nothing stopped two runs from using the same checkout at once, where one installs or writes the Rush scratch root
manifest while the other reads or times it.

## Decision Drivers

* A difference is counted once, where it arises
* A known deviation names its cause, is decided from data the harness has, and lists the categories it applies to
* Both tools see the same checkout and the same installed packages

## Considered Options

* Read the scope, run knip in the installed Rush checkout, and widen the deviations to the findings with the same cause
* Keep the deviations as they are and classify the clusters by hand per run

## Decision Outcome

Chosen option: "Read the scope, run knip in the installed Rush checkout, and widen the deviations to the findings with the same cause", because each widened deviation keeps a cause the harness checks from the reports, the manifests or the files, and the narrowed scope makes `not-judged-by-skif` exact instead of an upper bound.

**Scope of `not-judged-by-skif`.** A knip-only finding in `dependencies` or `devDependencies` of a package listed
under `incomplete-packages` is set apart whatever its `scope`. One in `files`, `exports`, `types`, `nsExports`,
`nsTypes`, `enumMembers` or `namespaceMembers` is set apart when the package's `scope` is `"package"`, or when it is
`{"within": [globs]}` and the file's path relative to the package directory matches one of the globs, as skif matches
them (`skif_js::IncompleteScopeGlobs`, compiled once per package). A directory listed twice gets the union of its
scopes.

**Rush.** rushstack is installed with `node common/scripts/install-run-rush.js install --bypass-policy`. For
`knip-root = "rush"` the harness writes the root `package.json` made from `rush.json` into the checkout while knip
runs there and removes it afterwards, so knip reads the `node_modules` Rush linked into each project and both reports
share one root. The manifest carries a `//` mark; one an interrupted run leaves is removed when the checkout is next
fetched, and the harness refuses to replace a root manifest without the mark. A finding only knip reports in a file of
a Rush root that lies in no package is the `rush-root-file` known deviation.

**Install and build commands.** A corpus repository may set `build-command` (program first), run in the checkout
after a successful install and before the install counts as done; a failed or timed-out build skips the repository
with the reason, as a failed install does. n8n builds `@n8n/oxlint-config`, `@n8n/vitest-config`,
`@n8n/eslint-config`, `@n8n/stylelint-config` and `@n8n/constants` with the workspace packages each depends on. The
install marker holds the pinned commit and, when the repository sets them, its `install-command` and `build-command`,
so a checkout installed with other commands is installed again. n8n keeps its `knip-invalid` mark until a parity run
confirms that knip loads those configurations.

**Checkout lock.** `corpus fetch`, `run`, `parity` and `syncpack-diff` take an exclusive lock on each corpus checkout
before fetching it and hold it until their pass over the repositories ends; `run` holds it until every target is
measured. The lock is the operating system's file lock (`std::fs::File::try_lock`) on `<name>.lock` beside the
checkout, the same on Linux, macOS and Windows. A second run fails at once, naming the lock file and the process id the
file holds. The operating system releases the lock when its process ends, so a run that dies leaves no stale lock and
no process-id check is needed; the file itself stays.

**`non-member-project`.** Every in-file category: a finding only knip reports in `files`, `exports`, `types`,
`nsExports`, `nsTypes`, `enumMembers`, `namespaceMembers`, `duplicates` or `unresolved` in a file below a project that
is not one of knip's workspaces, and one in `unlisted` there when the project's own manifest lists the package. The
manifest categories stay with `knip-unjudged-manifest`. When a recursive skif run lints the project as a package of one
of its roots ([ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md): a directory among the
`packages` of the JSON `roots`), skif judges the exports, types and members of its files, so `exports`, `types`,
`nsExports`, `nsTypes`, `enumMembers`, `namespaceMembers` and `duplicates` there stay compared; `files`, `unresolved`
and `unlisted` depend on the project's own entry files, plugins and manifest, which knip does not read, and stay set
apart.

**`used-in-knip-unused-file`.** A finding only knip reports in `dependencies`, `devDependencies` or
`optionalPeerDependencies` whose package a file of the same knip workspace imports, where knip lists that file under
`files` and skif does not report it unused. A file both tools report unused credits the package on neither side, so a
dependency only it imports stays compared. An `@types/` package counts as imported with its runtime package. The
harness parses the files knip lists as unused with `skif-js-syntax` and reads their bare specifiers (imports, type
imports, requires, re-exports and dynamic imports; not globs or test-runner mocks).

**`used-in-skif-unused-file`.** The mirror on skif's side, from the same import scan: a finding only skif reports in
`dependencies`, `devDependencies` or `optionalPeerDependencies` whose package a file of the same knip workspace
imports, where skif reports that file unused and knip does not. skif credits no import of a file it calls unused,
while knip analyses the file and credits the import.

**`published-source`.** A published path also names its source below `src` when its first directory is a build
directory as skif decides it (`skif_js::is_out_dir`: `dist`, `lib`, `out`, `esm` and the others, also with a flavour
suffix such as `lib-commonjs`), and a published path whose file name has no extension (`./out/main`) names any
TypeScript source with that stem. A published path without `*` names one source only, as skif's `map_to_source`
picks it: the first of `skif_js::source_candidates` that exists in the checkout, so with both `src/x.ts` and
`lib/x.ts` present, `./lib/x.js` names `src/x.ts` and `lib/x.ts` stays compared. A pattern with `*` names every file
it matches. The files that only such an entry reaches stay compared.

**Owning package.** The package a finding belongs to, for the narrowed scope of `not-judged-by-skif`, for
`knip-config-error`, `rush-root-file` and `published-source`, and for the `out`, `target` and `coverage` directories
of `skipped-directory`, is the deepest directory above its file with a project manifest as skif's file discovery
decides it: a `package.json` holding only `type`, `sideEffects` or `imports` (`{"type": "module"}` in `src/esm`) is a
marker for the files below it and belongs to the package above.

**Precedence.** For a finding only knip reports: `knip-unread`, the file classifications, the wider `fixture-file`,
`rush-root-file`, `not-judged-by-skif`, `skipped-directory`, `non-member-project`, `published-source`,
`in-skif-unused-file`, `used-in-knip-unused-file`, then `knip-config-error`. For a finding only skif reports:
`knip-unread`, `foreign-tsconfig-paths`, `knip-unjudged-manifest`, `in-knip-unused-file`, `used-in-skif-unused-file`,
then `knip-config-error`.

### Consequences

* Good, because `not-judged-by-skif` no longer hides the findings skif judged in a narrowed package.
* Good, because rushstack is compared with knip installed, as the other repositories are.
* Good, because the dependencies and in-file findings downstream of a `files` or project disagreement are counted once.
* Good, because two runs on one corpus directory cannot interleave, and the second says which process holds it.
* Bad, because the harness writes into the rushstack checkout while knip runs, and a run killed at that moment leaves
  a root manifest until the checkout is next fetched.
* Bad, because the import scan does not establish that the import it finds is the one skif credits, so a real skif
  difference there is hidden as long as the file disagreement lasts.
* Bad, because the category set changes, so every baseline entry has to be recorded again before `--check` compares it.
* Bad, because a `build-command` runs the repository's own build scripts, which the install without scripts avoids.

### Confirmation

* The harness unit tests in `tools/bench` set apart a knip-only file inside a narrowed scope and keep one outside it
  compared, treat dependencies as unjudged in any listed package, classify every in-file category below a non-member
  project, a dependency only a file one tool alone calls unused imports (on either side), a Rush root file and the sources of build-directory and
  extensionless published paths, and write and remove the Rush scratch root manifest; they record the install and
  build commands in the install marker and refuse a checkout whose lock another holder has, until it is released.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read the scope, run knip in the installed Rush checkout, and widen the deviations to the findings with the same cause

* Good, because each set-apart finding has a cause the harness checks.
* Bad, because more categories hide real differences while the underlying disagreement lasts.

### Keep the deviations as they are and classify the clusters by hand per run

* Good, because nothing more is set apart.
* Bad, because the same clusters (hundreds of rushstack findings, the narrowed packages of next-js) have to be
  re-classified after every run, and the counts keep mixing them with real differences.

## More Information

* Amends: [ADR 0080](0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md)
  (`non-member-project` covers every in-file category; `published-source` maps build directories and extensionless
  paths) and [ADR 0096](0096-knip-comparison-findings-in-unused-files-invalid-runs-and-two-sided-config-errors.md)
  (the scope of `not-judged-by-skif` is read)
* Related decisions: [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md),
  [ADR 0053](0053-incomplete-packages-in-the-json-report.md),
  [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md),
  [ADR 0068](0068-what-the-knip-comparison-sets-apart.md),
  [ADR 0081](0081-exports-outside-a-narrowed-incomplete-scope-are-judged.md)
* See the [ADR index](index.md).
