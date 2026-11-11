---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0009", "0089"]
---

# Stray Lockfiles a Build Installs From, npm's Shrinkwrap, and a Dependency in None of Several Catalogs

## Context and Problem Statement

[ADR 0089](0089-stray-lockfiles-and-nested-workspace-roots.md) has `stray-lockfile` report every lockfile in a
selected directory as a leftover, and accept both `package-lock.json` and `npm-shrinkwrap.json` at the root of an npm
workspace. Two cases contradict it:

* In a Bazel monorepo that uses rules_js, a member's own lockfile is often live. `npm_translate_lock(pnpm_lock =
  "//libs/a:pnpm-lock.yaml")` installs the package from it, and a Dockerfile that builds the member on its own copies
  it (`COPY package.json pnpm-lock.yaml ./`). The rule's unsafe fix deletes the file and breaks those builds, and with
  `[lint] fail-on-warnings = true` every such member fails CI (GitHub issue #180).
* npm ignores `package-lock.json` when `npm-shrinkwrap.json` is next to it, so the first file only goes stale, as
  Bun's `bun.lockb` does next to `bun.lock` ([ADR 0104](0104-bun-workspace-semantics-literal-members-lockfiles-nested-workspaces-and-patches.md),
  GitHub issue #157).

Separately, [ADR 0009](0009-version-and-catalog-policies.md) maps syncpack's `NotUsingCatalogAndCatalogUnknown` to
`catalog-not-used`. skif assigns that state both to a dependency several catalogs define and to one that no catalog
defines while several catalogs exist. The second is reported as "more than one catalog could hold it" under a rule
that says the dependency does not use its catalog, although it has no entry anywhere (GitHub issue #155).

Which member lockfiles are live, how does skif find out without running a build, and how is a dependency in none of
several catalogs reported?

## Decision Drivers

* The fix of `stray-lockfile` never deletes a file a build reads
* skif reads files and runs nothing: no Bazel query, no Docker build
* The cost stays bounded: directory listings and the files whose names match, read only when there is a member
  lockfile to judge
* Library crates read through `FileSystem` only, and the toolchain rules' scan is reused rather than repeated
* A finding names the state the dependency is in, under the rule whose name says it

## Considered Options

* Leave a lockfile a build names unreported, found by reading Bazel files and Dockerfiles
* Report such a lockfile at `info`, naming the consumer, so that `allow` keeps it on purpose
* Keep reporting it and rely on `allow`

## Decision Outcome

Chosen option: "Leave a lockfile a build names unreported, found by reading Bazel files and Dockerfiles", because a
lockfile a build installs from is not stray: there is nothing to fix, and an `info` finding or a required `allow`
entry would make every rules_js monorepo restate its build configuration in `skif.toml`.

**Consumers.** A lockfile below the root (the root's own lockfiles are judged as before) is not reported when one of
these names it:

* a Bazel label of the main repository, `//<dir>:<lockfile>` (also written `@//` or `@@//`), in a string literal of a
  `MODULE.bazel`, `REPO.bazel`, `WORKSPACE`, `WORKSPACE.bazel`, `*.bzl`, `BUILD` or `BUILD.bazel` file anywhere below
  the workspace root. A label is relative to the Bazel repository the root belongs to, the nearest directory at or
  above the root holding `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` or `WORKSPACE`; when that directory is above
  the root, the Bazel files directly in it are read too, and a label outside the workspace names none of its files. A
  label of another repository (`@npm//...`) names nothing;
* a string literal that is the lockfile's name or a relative label (`"pnpm-lock.yaml"`, `":pnpm-lock.yaml"`) in a
  `BUILD` or `BUILD.bazel` file of the lockfile's own directory, as `exports_files` or `srcs` list it;
* a `COPY` or `ADD` source in a Dockerfile (`Dockerfile`, `Dockerfile.*`, `*.Dockerfile`, `Containerfile`) of the
  lockfile's own directory that names the lockfile, or a glob that matches it (`pnpm-lock.yaml*`). The directory is
  taken as the build context. An instruction with `--from` copies from a stage or an image and does not count, and
  neither does a source that is a directory (`COPY . .`).

Starlark `#` comments are skipped. Bazel files are found by one walk below the root that lists names only, skips
`node_modules`, `.git` and symbolic links (Bazel's `bazel-*` output links among them) and stops 16 directories down,
as the `files` globs of the toolchain rules walk; only the files whose names match are read. Dockerfiles come from the
scan the toolchain rules share ([ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md)),
which records the context sources of each `COPY` and `ADD`. Neither is read when the workspace has no lockfile below
the root. An `allow` entry that names a lockfile a build names is reported as stale, as any entry that matches no file
the rule would report.

**npm's shrinkwrap.** In an npm workspace, a root `package-lock.json` next to an `npm-shrinkwrap.json` is reported,
with the same unsafe fix: npm installs from `npm-shrinkwrap.json`. Either file alone is still the lockfile npm reads.

**A dependency in none of several catalogs.** With a catalog mode set and more than one catalog defined, a dependency
used in at least `min-packages` packages that no catalog defines is reported by `catalog-missing-entry` with the state
`MissingFromCatalogAndCatalogUnknown` and the message "no catalog holds `<name>`; more than one could". There is no
fix, because skif does not choose the catalog. `NotUsingCatalogAndCatalogUnknown` (`catalog-not-used`) is left to a
dependency that several catalogs define. This departs from syncpack, which assigns its one state to both.

### Consequences

* Good, because `--fix-unsafe` no longer deletes the lockfile of a member that Bazel or a Dockerfile installs, and an
  upgrade does not fail CI on every such member.
* Good, because the detection reads only files whose names say they are build files, and only when a member lockfile
  exists.
* Good, because a `package-lock.json` that npm ignores is reported like Bun's ignored `bun.lockb`.
* Good, because a dependency without a catalog entry is always a `catalog-missing-entry` finding.
* Bad, because a consumer written another way is not seen: a label built from variables, a `glob()`, a Dockerfile in
  another directory whose context is the member, or a build script that copies the file. Such a lockfile keeps an
  `allow` entry.
* Bad, because a commented-out label inside a multi-line string still counts.
* Neutral, because the walk lists every directory below the root once per run when a member lockfile exists.

### Confirmation

* `crates/skif-rules/src/manifest/lockfile_consumers.rs` tests: string literals and comments, main-repository labels
  and other repositories' labels, `BUILD` listings, the recognised file names, labels relative to a Bazel root above
  the workspace, and Dockerfile copies by name and glob.
* `crates/skif-rules/src/manifest/stray_lockfile.rs` tests: the rules_js repro with one member installed by Bazel and
  one by its Dockerfile, `BUILD` files of the lockfile's directory, a Dockerfile elsewhere or copying from a stage,
  the stale `allow` entry, and `package-lock.json` next to `npm-shrinkwrap.json`.
* `crates/skif-rules/src/toolchain/parse.rs`: `dockerfile_copies_from_the_build_context_are_recorded`.
* `crates/skif-rules/src/policy/catalog/missing_entry.rs` tests and the fixture
  `fixtures/policy/catalog/ambiguous-dependency-in-no-catalog`: the new state under pnpm and Bun.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Leave a lockfile a build names unreported, found by reading Bazel files and Dockerfiles

* Good, because a live lockfile produces no finding and no fix.
* Bad, because the rule reads files beyond the workspace facts of ADR 0089.

### Report such a lockfile at `info`, naming the consumer, so that `allow` keeps it on purpose

* Good, because the consumer is visible in the report.
* Bad, because a file with nothing wrong is reported, and silencing it takes an `allow` entry per member.

### Keep reporting it and rely on `allow`

* Good, because the rule stays as simple as ADR 0089 has it.
* Bad, because every rules_js monorepo must list its member lockfiles before it can upgrade, and the unsafe fix stays
  destructive for those that do not.

## More Information

* Amends: [ADR 0089](0089-stray-lockfiles-and-nested-workspace-roots.md) (`stray-lockfile`: a lockfile a build names
  is not reported; npm's `package-lock.json` next to `npm-shrinkwrap.json` is) and
  [ADR 0009](0009-version-and-catalog-policies.md) (the state `MissingFromCatalogAndCatalogUnknown` of
  `catalog-missing-entry`)
* rules_js `npm_translate_lock`: <https://github.com/aspect-build/rules_js/blob/main/docs/npm_translate_lock.md>
* npm shrinkwrap: <https://docs.npmjs.com/cli/configuring-npm/npm-shrinkwrap-json>
* See the [ADR index](index.md).
