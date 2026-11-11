---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0097"]
---

# Member Binaries Under pnpm's Hoisted Node Linker

## Context and Problem Statement

[ADR 0097](0097-commands-in-other-directories-and-binaries-of-workspace-members.md) provides a workspace member's
binary to a package that does not list the member only when the package manager links every member's binaries into
the root `node_modules/.bin`. It takes pnpm to be isolated whatever its settings, and lists a pnpm workspace with
`node-linker=hoisted` as a known false finding.

pnpm's hoisted linker builds a flat `node_modules` as npm does, so a member's binary is on the `PATH` of every
package's scripts. A workspace with `nodeLinker: hoisted` in `pnpm-workspace.yaml`, or before pnpm 11
`node-linker=hoisted` in the root `.npmrc`, has `unlisted-binaries` report a script that runs a member's binary
although the install provides it (GitHub issue #146).

Which pnpm settings should decide whether a member's binaries are on every package's `PATH`?

## Decision Drivers

* A binary must be judged against the binaries the install puts on the `PATH` where it runs
* Only settings committed with the repository are read; an analysis must not depend on the user's machine
* An isolated pnpm workspace keeps reporting a member's binary that a package does not list

## Considered Options

* Read `nodeLinker` of `pnpm-workspace.yaml`, else `node-linker` of the root `.npmrc` before pnpm 11
* Keep pnpm isolated and leave hoisted workspaces to `ignore-binaries`

## Decision Outcome

Chosen option: "Read `nodeLinker` of `pnpm-workspace.yaml`, else `node-linker` of the root `.npmrc` before pnpm 11",
because these are the places a repository sets pnpm's linker, and with the hoisted linker pnpm puts member binaries
where npm does. pnpm 11 reads only auth and registry settings from `.npmrc`, and its `node_modules` layout settings
only from `pnpm-workspace.yaml`.

Under pnpm, a member's binary is provided to every package when pnpm's linker is `hoisted`, as under npm and Yarn
Classic; otherwise the answer of ADR 0097 stands. The linker is `nodeLinker` in `pnpm-workspace.yaml` when it is a
string; taking it over `.npmrc` is skif's choice. Without one, the value of the last top-level `node-linker` entry of
the root `.npmrc` decides, read as npm's `ini` parser reads it (trimmed, unquoted, without a byte order mark or an
inline comment, and nothing under a `[section]`), but only when the root's `packageManager` names a pnpm before 11 or
no pnpm version: under pnpm 11 and later `.npmrc` is not read. Without a declared version skif cannot tell pnpm 10
from 11, so it reads `.npmrc`. Without either setting, pnpm's default, `isolated`, applies.
`isolated` and `pnp` keep a member's binary to the packages that list the member. A Rush repository stays isolated
whatever its pnpm settings.

A member's `.npmrc`, the user's `.npmrc`, `npm_config_node_linker` and `pnpm install --node-linker` are not read.
`shamefully-hoist` and `public-hoist-pattern` are not read either: they hoist the dependencies of the isolated layout
into the root `node_modules`, and under every package manager skif credits an external binary only to a package that,
itself or through an ancestor, lists the dependency providing it, never through a dependency of another package.

### Consequences

* Good, because a pnpm workspace with the hoisted linker, set in either file that its pnpm reads, no longer reports a
  member's binary the install provides.
* Good, because the isolated default, npm, Yarn and Bun keep every earlier answer.
* Bad, because a linker that only the user's `.npmrc`, the environment or the command line sets is not seen, so such
  a workspace keeps the answer its committed files give.
* Bad, because a pnpm 11 workspace that declares no pnpm version in `packageManager` and keeps a stale
  `node-linker=hoisted` in `.npmrc` is taken for hoisted.

### Confirmation

* `crates/skif-workspace/src/pnpm.rs`: `node_linker_is_read_when_it_is_a_string`.
* `crates/skif-workspace/src/npmrc.rs`: the `.npmrc` entry tests (trimming and quotes, comments, a byte order mark,
  `[section]` headers, inline comments).
* `crates/skif-js/tests/monorepo_regressions.rs`: `pnpm_links_member_binaries_as_its_node_linker_says` (the
  isolated default, `nodeLinker: hoisted`, `node-linker=hoisted` with and without spaces, quotes, a byte order mark
  or an inline comment, the last `.npmrc` line winning, a commented-out line, one under a section, `pnp`, a member's
  `.npmrc`, `pnpm-workspace.yaml` overriding `.npmrc` both ways, pnpm 11 ignoring `.npmrc` but not
  `pnpm-workspace.yaml`, and an undeclared pnpm version reading `.npmrc`), and
  `a_member_binary_is_provided_where_the_package_manager_links_it` unchanged.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read `nodeLinker` of `pnpm-workspace.yaml`, else `node-linker` of the root `.npmrc` before pnpm 11

* Good, because the answer follows the linker the repository configures, in the files its pnpm reads.
* Bad, because a setting outside the repository is not seen.

### Keep pnpm isolated and leave hoisted workspaces to `ignore-binaries`

* Good, because nothing changes.
* Bad, because every member binary of a hoisted workspace needs an entry, and a binary that later goes missing stays
  unreported.

## More Information

* Amends: [ADR 0097](0097-commands-in-other-directories-and-binaries-of-workspace-members.md) (the "Member binaries"
  paragraph for pnpm, and its consequence on `node-linker=hoisted`)
* Issue: [#146](https://github.com/Goldziher/skif/issues/146)
* pnpm settings: <https://pnpm.io/settings#nodelinker>
* pnpm 11 release notes (`.npmrc` keeps only auth and registry settings):
  <https://github.com/pnpm/pnpm/releases/tag/v11.0.0>
* See the [ADR index](index.md).
