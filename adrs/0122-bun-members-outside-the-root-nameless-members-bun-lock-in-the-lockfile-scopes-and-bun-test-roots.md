---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0090", "0104"]
---

# Bun Members Outside the Root, Nameless Members, bun.lock in the Lockfile Scopes, and Bun Test Roots

## Context and Problem Statement

[ADR 0104](0104-bun-workspace-semantics-literal-members-lockfiles-nested-workspaces-and-patches.md) follows Bun where
its workspaces differ from pnpm's. Checked against `bun install` and `bun test` of Bun 1.4.4, several differences were
still open (GitHub issue #158):

* Bun installs a member outside the root (`"workspaces": ["../shared"]`) and records it in `bun.lock` as
  `"../shared"`. skif reported the pattern as an `invalid-workspace-file` error.
* Bun skips a member whose `package.json` has no `name`, without a warning, so none of its dependencies are installed.
  skif linted it as a member.
* [ADR 0090](0090-lockfile-scopes-for-banned-groups-and-dependency-families.md) reads only `pnpm-lock.yaml` for the
  lockfile scopes of `banned-dependency` and `unsynced-dependency-family`. In a Bun workspace both reported that
  nothing was checked.
* The broken-root fallback (`declared_member_dirs`) let a negation remove a literal member that discovery keeps under
  Bun.
* `bun test` looks only below `[test] root` of `bunfig.toml` and leaves out what `pathIgnorePatterns` matches. Both
  globs are relative to the config, not to the root. The plugin's test entries ignored both, so a file `bun test` never
  runs counted as used.
* `bunfig.toml`'s `[lint] plugins` names packages and files that `bun lint` loads.
* `Bun.build` resolves a literal entry point from the working directory, but one computed from `import.meta.dir` from
  the file. skif resolved both from the package.
* Bun ignores `resolutions` whenever `overrides` is present, even an empty one.

What does skif do with each of these?

## Decision Drivers

* skif's members and findings match what `bun install` and `bun test` do
* Every other package manager keeps its behaviour
* The lockfile checks never pass silently

## Considered Options

* Follow Bun in each case, keyed on the detected package manager, and document the `resolutions` behaviour
* Lint members outside the root as members too
* Leave the cases open and point users at `ignore-packages`, `ignore-rules` and `[js.plugins]`

## Decision Outcome

Chosen option: "Follow Bun in each case, keyed on the detected package manager, and document the `resolutions`
behaviour", because each case is a false or a missed finding that the repository's files settle.

**Members outside the root.** In a Bun workspace a positive pattern may start with `..` segments (`../shared`,
`../libs/*`). It expands from the directory those segments name. The directories it selects count for the pattern's
`matched`, so it is neither invalid nor unmatched. Those with a named `package.json` are listed in
`DiscoveryFacts::outside_root_members`, root-relative. They are not loaded as packages: everything skif lints lies
below the root. A `..` after another segment, a `..` in a negation, and every `..` under another manager still cannot
be used.

**Members without a name.** In a Bun workspace a member whose `package.json` has no `name`, or an empty one, is listed
in `DiscoveryFacts::nameless_members`. It stays a package and is excluded from analysis as `ignore-packages` would
exclude it, like a member inside a Bazel convenience link (ADR 0116). A member that configuration or a Bazel link
excludes already is not listed. `missing-package-manifest` reports each listed member at its manifest (at the `name`
key when there is one), with the detail `NamelessBunMember`. The unsafe fix names the member after its directory. The
rule's summary names the case.

**bun.lock in the lockfile scopes.** In a Bun workspace the shared lockfile of the rules is `bun.lock`. This amends the
lockfile model of ADR 0090. `BunLockfile` models the `packages` entries as snapshots keyed by their `packages` key
(`ms`, `debug/ms`, `app/ms`), workspace packages left out. A dependency resolves as Bun looks it up: below the
package's own key, then below each key it is nested in, then at the top. The importers are the `workspaces` entries
(`.` for the root), and their `dependencies`, `devDependencies` and `optionalDependencies` resolve below the workspace
package's name first. `BunLockfile::resolutions` walks them breadth-first like `PnpmLockfile::resolutions`. Findings
name the release (`ms@2.1.2`), not the key, and point at the key of the `packages` entry. Without a `bun.lock`, with
only a `bun.lockb`, or with a `bun.lock` that does not parse, the check reports that nothing was checked, as ADR 0090
has it. `stale-patched-dependencies` keeps its own reading of ADR 0104.

**The broken-root fallback.** `declared_member_dirs` applies Bun's literal rule when it detects Bun.

**Bun test roots.** A `bunfig.toml` that sets `[test] root` or `pathIgnorePatterns` replaces the plugin's default test
globs, through the new `PluginInput::ReplaceDefaultEntries`. A root rebases the globs below it, relative to the config.
With ignore patterns the matching files are listed, and those a pattern matches, or that lie in a directory a pattern
matches, are left out. Entry globs cannot leave files out. A root that leaves the package is not followed. Entries set
for the plugin in `[js.plugins]` replace the defaults already and win. The plugin's other entries stay additive.

**Lint plugins and file-relative build entries.** `[lint] plugins` is read like `preload`: a package is a dependency,
a path an entry. A command call (`extract_command_calls` of `skif-js-syntax`) marks each `entrypoints` string that the
file's own location computes with `FILE_DIR`, using the probe evaluation of the `cwd` option. The Bun plugin resolves
such an entry point from the calling file's directory.

**`resolutions` next to `overrides`.** Not reported. The configuration reference says that Bun ignores `resolutions`
while `overrides` exists. skif still reads both as instances.

### Consequences

* Good, because a Bun workspace with a shared directory beside it, a nameless member, a lockfile policy or a narrowed
  `bun test` gets the findings `bun install` and `bun test` imply.
* Good, because other managers and Bun workspaces without these features keep their reports.
* Bad, because members outside the root are not linted at all; their directory has to be linted as a root of its own.
* Bad, because ignore patterns cost a walk of the package for test files, up to 32 directories deep.
* Neutral, because `bun.lockb`-only workspaces still get no lockfile check.

### Confirmation

* `crates/skif-workspace/tests/memory.rs`: members outside the root under Bun and npm, and nameless members with and
  without `ignore-packages`.
* `crates/skif-workspace/src/pattern.rs`, `roots.rs` and `bun_lock.rs` tests: leading `..` segments, the literal rule of
  the fallback, and the graph of a `bun.lock` that `bun install` 1.4.4 wrote.
* `crates/skif-rules` tests of `missing-package-manifest`, `unmatched-workspace-patterns`, `banned-dependency` and
  `unsynced-dependency-family`: the Bun cases.
* `crates/skif-js-plugins/tests/plugins.rs` and `crates/skif-js-syntax/tests/imported_calls.rs`: test roots, ignore
  patterns, lint plugins and file-relative entry points.

## Pros and Cons of the Options

### Follow Bun in each case, keyed on the detected package manager, and document the `resolutions` behaviour

* Good, because each finding matches the install.
* Bad, because discovery, the lockfile loader and the Bun plugin carry more Bun branches.

### Lint members outside the root as members too

* Good, because those members would be linted with the workspace.
* Bad, because every path, the JS analysis and the fixes assume that packages lie below the root.

### Leave the cases open and point users at `ignore-packages`, `ignore-rules` and `[js.plugins]`

* Good, because nothing changes.
* Bad, because an error on a working workspace, a member Bun never installs and two lockfile checks that cannot run
  stay, and each needs configuration to work around.

## More Information

* Bun workspaces: <https://bun.com/docs/install/workspaces>
* Bun lockfile: <https://bun.com/docs/install/lockfile>
* Bun overrides: <https://bun.com/docs/install/overrides>
* `bunfig.toml`: <https://bun.com/docs/runtime/bunfig>
