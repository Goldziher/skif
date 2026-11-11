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
package's scripts. A workspace with `nodeLinker: hoisted` in `pnpm-workspace.yaml`, or `node-linker=hoisted` in the
root `.npmrc`, has `unlisted-binaries` report a script that runs a member's binary although the install provides it
(GitHub issue #146).

Which pnpm settings should decide whether a member's binaries are on every package's `PATH`?

## Decision Drivers

* A binary must be judged against the binaries the install puts on the `PATH` where it runs
* Only settings committed with the repository are read; an analysis must not depend on the user's machine
* An isolated pnpm workspace keeps reporting a member's binary that a package does not list

## Considered Options

* Read `nodeLinker` of `pnpm-workspace.yaml`, else `node-linker` of the root `.npmrc`
* Keep pnpm isolated and leave hoisted workspaces to `ignore-binaries`

## Decision Outcome

Chosen option: "Read `nodeLinker` of `pnpm-workspace.yaml`, else `node-linker` of the root `.npmrc`", because these are
the two places a repository sets pnpm's linker, and with the hoisted linker pnpm puts member binaries where npm does.

Under pnpm, a member's binary is provided to every package when pnpm's linker is `hoisted`, as under npm and Yarn
Classic; otherwise the answer of ADR 0097 stands. The linker is `nodeLinker` in `pnpm-workspace.yaml` when it is a
string, since pnpm lets the settings of `pnpm-workspace.yaml` override `.npmrc`; without one, the value of the last
`node-linker=` line of the root `.npmrc`, trimmed and unquoted. Without either, pnpm's default, `isolated`, applies.
`isolated` and `pnp` keep a member's binary to the packages that list the member. A Rush repository stays isolated
whatever its pnpm settings.

The user's `.npmrc`, `npm_config_node_linker` and `pnpm install --node-linker` are not read. `shamefully-hoist` and
`public-hoist-pattern` are not read either: they hoist the dependencies of the isolated layout into the root
`node_modules`, and under every package manager skif credits an external binary only to a package that, itself or
through an ancestor, lists the dependency providing it, never through a dependency of another package.

### Consequences

* Good, because a pnpm workspace with the hoisted linker, set in either file, no longer reports a member's binary the
  install provides.
* Good, because the isolated default, npm, Yarn and Bun keep every earlier answer.
* Bad, because a linker that only the user's `.npmrc`, the environment or the command line sets is not seen, so such
  a workspace keeps the answer its committed files give.

### Confirmation

* `crates/skif-workspace/src/pnpm.rs`: `node_linker_is_read_when_it_is_a_string`.
* `crates/skif-js/tests/monorepo_regressions.rs`: `pnpm_links_member_binaries_as_its_node_linker_says` (the
  isolated default, `nodeLinker: hoisted`, `node-linker=hoisted` with and without spaces and quotes, the last
  `.npmrc` line winning, a commented-out line, `pnp`, and `pnpm-workspace.yaml` overriding `.npmrc` both ways), and
  `a_member_binary_is_provided_where_the_package_manager_links_it` unchanged.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read `nodeLinker` of `pnpm-workspace.yaml`, else `node-linker` of the root `.npmrc`

* Good, because the answer follows the linker the repository configures, in pnpm's own precedence.
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
* See the [ADR index](index.md).
