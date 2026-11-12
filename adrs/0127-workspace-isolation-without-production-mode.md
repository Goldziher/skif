---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Workspace Isolation Without Production Mode

## Context and Problem Statement

`skif --strict` is knip's strict mode: production mode, each workspace package limited to the dependencies its own
`package.json` lists, and knip's rules for direct dependencies (production code imports from `dependencies` or
`peerDependencies`, a peer dependency of a listed package does not stand in for a listing, type-only imports use no
package, required peer dependencies are checked like `dependencies`). It is the only way to isolate workspaces, and a
file cannot set it. A project that wants isolated dependency checks for all of its code, tests, test setup and tool
configs included, runs skif twice: once in the default mode for the development dependencies and once with
`--strict`. The two runs disagree on configuration hints, since an `ignore-dependencies` entry for a development-only
false positive is used in the default run and reported by `stale-js-config` in the strict run, which does not analyse
that file; and `skif baseline prune` in one mode removes the entries of the other (GitHub issue #221).

knip keeps all of these effects under its one `isStrict` option: `DependencyDeputy` limits the workspaces it looks a
dependency up in to the importing one, drops the exception for a private package's workspace members and for peers of
listed packages, and requires `dependencies` for production code, and the graph builder skips type-only imports. Its
`--isolate-workspaces` flag (knip 5, dropped in knip 6, where it is always on) built one TypeScript program per
workspace, a memory and speed setting that changes no finding; knip has no configuration key for either.

How can a project isolate workspaces without production mode, and set the mode in its configuration?

## Decision Drivers

* One run can check every package, its development files included, against its own manifest
* `--strict` keeps knip's answer
* A mode that CI depends on can live in the configuration, so `skif baseline write` and `prune` see it too
* Isolation means one thing: which manifests count, not which entries of them

## Considered Options

* `[js] isolate-workspaces` for which manifests count, `[js] strict` for knip's strict mode, which implies it
* `[js] isolate-workspaces` as all of knip's strict mode except production mode
* Only `[js] strict`, the configuration counterpart of the flag

## Decision Outcome

Chosen option: "`[js] isolate-workspaces` for which manifests count, `[js] strict` for knip's strict mode, which
implies it", because isolation without production mode is useful only when the default mode's rules for development
files stay as they are, and strict mode keeps knip's answer unchanged.

`isolate-workspaces` (default `false`, top level only, also `skif --isolate-workspaces`) makes each workspace package
use only the dependencies its own manifest lists. An import, a binary a script runs, and a package a tool config names
are no longer satisfied by a containing package or the root; a private package's import of a workspace member it does
not list is reported, and `use-public-hoisting` is ignored, since a hoist is a listing of the root. Manifests above the
analysed root still count, as in strict mode. Nothing else changes: outside production mode every file is analysed
and tool configs are judged against the manifest of their own package, any dependency field satisfies an import, a
type-only import needs a listing (a listed `@types` package stands in), and a peer dependency of a listed package
stands in for a listing.

`strict` (default `false`, top level only) is `skif --strict` in the configuration: `production`, `isolate-workspaces`
and the direct-dependency rules above. With it on, `production = false` and `isolate-workspaces = false` have no
effect. The command line can turn each mode on, not off, as for `production`. In the library, `Js::isolate_workspaces`
is now isolation only, and `Js::strict` holds strict mode; `Js::production_mode` and `Js::isolated` give the effective
modes.

`skif migrate --from knip` writes `[js] strict = true` when every script runs knip with `--strict`, and leaves it to a
human when only some do, as for `--production`. knip's `--isolate-workspaces` stays without effect: it is not
isolation.

A project that ran a default and a strict run can run once with `isolate-workspaces = true`. Its `ignore-dependencies`
entries are judged against that one run, so `stale-js-config` no longer reports an entry the other run needs, and one
baseline covers the run. Running both modes still gives two sets of findings, and a baseline still belongs to one mode.

### Consequences

* Good, because one default run checks every package, tests and tool configs included, against its own manifest.
* Good, because `--strict` and knip's strict mode are unchanged, and a file can now ask for it.
* Good, because a mode set in the configuration is the mode of every run, `baseline write` and `prune` included.
* Bad, because isolation without production mode has no knip counterpart to compare with in the parity harness.
* Bad, because an embedder that set `Js::isolate_workspaces` for strict mode gets isolation only until it sets
  `Js::strict`.

### Confirmation

* `crates/skif-js/tests/isolate_workspaces.rs`: in the default mode isolation reports a member's import of a root-only
  development dependency, a type-only import of one, an unlisted workspace member of a private package and a `tsconfig`
  that extends a root-only package, keeps the package's own `devDependencies`, and reports the root's development
  dependency that only members use as unused; strict mode adds the direct-dependency rules and judges no config.
* `crates/skif-cli/tests/filters.rs`: `isolate_workspaces_reports_a_members_use_of_an_ancestor_only_dev_dependency`,
  `isolate_workspaces_is_not_strict_mode` and
  `strict_and_isolate_workspaces_in_the_configuration_equal_their_flags`.
* `crates/skif-config/tests/config.rs`: both keys parse, default to `false`, are rejected in an override, and `strict`
  implies production mode and isolation.
* `crates/skif-migrate/tests/golden.rs`: `knip_strict_is_written_to_js_strict_when_every_run_is_strict` and
  `knip_isolate_workspaces_flag_is_not_workspace_isolation`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### `[js] isolate-workspaces` for which manifests count, `[js] strict` for knip's strict mode, which implies it

* Good, because each key means one thing, and the two compose into knip's strict mode.
* Good, because development files keep the default mode's rules, which is what a run outside production mode is for.
* Bad, because two keys where knip has one option.

### `[js] isolate-workspaces` as all of knip's strict mode except production mode

* Good, because it is what `Js::isolate_workspaces` meant for an embedder.
* Bad, because strict mode treats every file but a declaration file as production code, so in the default mode a test
  importing its package's own `devDependencies` would be reported, and type-only imports would stop needing a listing.

### Only `[js] strict`, the configuration counterpart of the flag

* Good, because it adds one key and no new mode.
* Bad, because the project in the problem statement still needs two runs.

## More Information

* Issue: [#221](https://github.com/Goldziher/skif/issues/221)
* Related: [ADR 0086](0086-packages-tool-configs-name-are-unlisted-at-the-config.md) (configs judged against their
  own package), [ADR 0093](0093-baseline-entries-follow-what-a-finding-is-about.md) (baselines follow the lint
  flags), [ADR 0123](0123-publicly-hoisted-packages-as-available-dependencies.md) (public hoisting)
* knip: `packages/knip/src/DependencyDeputy.ts` (`isStrict`), `packages/knip/src/graph/build.ts` (`skipTypeOnly`),
  the knip 6 release notes (`--isolate-workspaces` dropped)
* See the [ADR index](index.md).
