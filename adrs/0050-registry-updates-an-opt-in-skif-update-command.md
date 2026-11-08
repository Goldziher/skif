---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
---

# Registry Updates: An Opt-In `skif update` Command, the Only Code That Uses the Network

## Context and Problem Statement

[ADR 0001](0001-mission-and-scope.md) puts syncpack's `update` out of scope: skif "never contacts a package registry: it opens no network connection and resolves no version", and [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) states that exactly. [ADR 0009](0009-version-and-catalog-policies.md) maps syncpack's `DiffersToNpmRegistry` state to no rule for the same reason. Issue [#34](https://github.com/Goldziher/skif/issues/34) asks for the missing half of syncpack parity: `update`, `--check`, `--target`, `--interactive`, `updateGroups` and `minimumReleaseAge`. A team that moves from syncpack to skif keeps syncpack installed for `syncpack update --check` in CI, which is the second Node install skif exists to remove. This record decides whether skif talks to a registry, where that code lives, and how it behaves. syncpack was read at tag 15.3.3 (`src/commands/update.rs`, `src/registry/`, `src/rcfile/update_group.rs`, the `PreferredSemver`, `CatalogDefs` and `SemverRangeOnly` visitors).

## Decision Drivers

* Linting must stay offline, deterministic and safe on untrusted pull requests
* The library must keep doing no I/O ([ADR 0003](0003-library-first-embedding.md)): a host such as poly must be able to plan updates through its own client, or a cache, or not at all
* syncpack semantics for which instances move and how far, so `skif migrate` can translate `updateGroups` and `minimumReleaseAge` one to one
* Private registries are common in monorepos; credentials must never reach a log, a report or the wrong host
* Every exit path testable without a real registry
* A small dependency footprint that passes `deny.toml` and builds without a C toolchain

## Considered Options

* An opt-in `skif update` subcommand: the library plans through a `Registry` trait, the CLI does HTTP
* Keep updates out of scope and document `syncpack update` alongside skif
* A lint rule that reports `DiffersToNpmRegistry`, behind a flag that enables the network
* Shell out to `npm view` / `pnpm outdated`

## Decision Outcome

Chosen option: "An opt-in `skif update` subcommand: the library plans through a `Registry` trait, the CLI does HTTP", because it gives syncpack's behaviour to users who ask for it while every other command keeps the offline guarantee, and because the network sits behind one trait that tests and hosts replace.

This record supersedes the "Registry updates and an interactive upgrade TUI" bullet of ADR 0001 and the "skif's own process never opens a network connection" statement of ADR 0016 for the `update` subcommand only, and the `DiffersToNpmRegistry` row of ADR 0009. For linting, `--fix` and every other subcommand those statements still hold.

### Network position

`skif update` is the only code in skif that opens a connection. Linting, `--fix`, `migrate`, `config`, `explain-usage` and the library never do; the library cannot, because it has no HTTP client.

* **What is requested.** `GET <registry><name>` with `Accept: application/json` (the full document: the abbreviated one has no publish times, which `minimum-release-age` needs), once per distinct dependency name, only for the instances that may move and that `--dependencies` selects. Nothing else is sent: no telemetry, no lockfile, no package list.
* **Which registry.** `npm_config_registry`, then `registry` in the project's `.npmrc`, then the user's `.npmrc` (`NPM_CONFIG_USERCONFIG`, else `~/.npmrc`), then `https://registry.npmjs.org/`. `@scope:registry` chooses per scope; `@jsr/*` goes to `https://npm.jsr.io/` unless configured, as syncpack does. syncpack ignores `.npmrc` entirely and fails on private registries (its issue 220); reading it is a deliberate improvement.
* **Credentials.** `//host/path/:_authToken` entries are read, with `${VAR}` expanded from the environment; an unset variable sends no token rather than a partial one. A token is sent only as `Authorization: Bearer` to the registry whose URL it is configured for (longest `//host/path/` prefix), and appears in no message, report or log. Credentials embedded in a registry URL are stripped, so the URLs in error messages are safe to print. `_auth`, username and password, client certificates, `cafile` and `strict-ssl` are out of scope; a team that needs them configures a registry proxy.
* **Transport.** `ureq` 3 (blocking, small, no async runtime) over rustls with the ring provider and the operating system's trust store through `rustls-native-certs` (`SSL_CERT_FILE` and `SSL_CERT_DIR` replace it); `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` are honoured; gzip. `webpki-roots` and `rustls-platform-verifier` are not used: their CDLA-licensed root bundle is not on the `deny.toml` allow-list, and a corporate interception proxy needs the system store anyway. ring 0.17 brings a second `windows-sys`, recorded as a skip in `deny.toml`.
* **Limits.** A 10 second connect timeout, 60 seconds per request, a 64 MiB body limit, and at most `[update] max-concurrent-requests` (default 12, syncpack's `maxConcurrentRequests`) requests at once.
* **Offline and failures.** A package the registry cannot answer for (unreachable, a non-200 status, a 404 for a private name without a token, an unreadable body) is a `failure`; its instances are left alone, the others are still planned. The run then exits `2`, because it is incomplete. With no network at all every name fails, nothing is written, and the exit code is `2`.

### Library and CLI split

`skif-rules` gains a pure `update` module, re-exported by the facade as `skif::update` behind a new `update` feature (implies `manifest` and `fix`):

* `trait Registry: Sync { fn fetch(&self, name) -> Result<Packument, FetchError>; fn fetch_all(&self, names) -> Vec<...> }`; the default `fetch_all` asks one by one, the CLI overrides it with a bounded thread pool.
* `Packument::from_json` reads a registry document (versions, `deprecated`, `time`), so a host that does its own HTTP only fetches text.
* `plan_updates(workspace, config, registry, options) -> UpdateReport` with `UpdateOptions { target, now, dependencies }`; `now` is passed in because the library reads no clock.
* `apply_updates(workspace, report, fs) -> FixPlan` plans the edits through the fix planner of [ADR 0008](0008-fix-and-edit-model.md), so writes are format-preserving and the CLI writes them as it writes fixes.

The HTTP client, `.npmrc` reading and the clock live in `skif-cli`, which is already the only crate that reads the environment.

### Which instances move

As syncpack 15.3.3, in skif's model:

* An instance owned by a version group that prefers the highest version (the catch-all included); not a `pinned`, `banned`, `ignored`, `snapped-to`, `same-range`, `same-minor` or lowest-preferring group.
* Not a workspace package, not a package's own version, and not a dependency whose group has a `catalog:` instance: the catalog entry moves instead. An instance the catalog rules own ([ADR 0040](0040-version-groups-own-their-instances-the-catalog-rules-leave-claimed-instances.md)) is not in the catch-all and does not move.
* A specifier syncpack updates: an exact version, a single-comparator range, a bare major or minor, `latest`, or an `npm:` alias of the dependency itself; a plain package name (no `a>b` or `name@range` override selectors).
* Catalog entries of `pnpm-workspace.yaml`, the root `package.json` (bun) and `.yarnrc.yml`, written where they are declared.
* Which sections count follows `[policy] dependency-types`: peer and optional dependencies, the override sections and custom types move only when named. syncpack updates every type by default; skif keeps one scope for all policies.

The new version is the highest published one that is not deprecated, is older than the minimum release age, has the same release channel as the current one, and is an eligible update under the effective target (syncpack's `is_eligible_update_for` and `has_same_release_channel_as`, already vendored in `skif-specifier`). It keeps the current range style unless a `[[semver-groups]]` entry asks for another.

### Configuration

* `--target latest|minor|patch` (default `latest`).
* `[[update-groups]]`, first match wins, with the selectors of `[[semver-groups]]` and exactly one of `target` or `ignored = true` (syncpack's `updateGroups` with `target` or `isIgnored`). The effective target is the stricter of `--target` and the group's. A catalog entry belongs to no package and has no dependency type, so groups that name `packages` or `dependency-types` do not match it. The list is prepended by nearer files and resettable with `reset = ["update-groups"]`, like the other group lists.
* `[update] minimum-release-age` in minutes (`0` turns it off); unset, pnpm's `minimumReleaseAge` from `pnpm-workspace.yaml` applies, else 1440, as in syncpack. `[update] max-concurrent-requests`.
* Both are root-scoped: an `[[overrides]]` entry or a package file cannot set them.

### `--check`, writing and exit codes

`skif update --check` writes nothing and exits `1` when an update is available, `0` when none is, `2` when a package could not be checked. Without `--check`, every update is written atomically per file, then the package manager's install runs as after `--fix` ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)) unless `--no-install`; writing is refused with exit `2` (`ci-refusal`) when `CI` is truthy, as `--fix` is. `--format json` prints one document with each update's path, position, package, dependency, place, current and target specifier and `state: "DiffersToNpmRegistry"`, plus the failures.

### `DiffersToNpmRegistry`

The state is reported by `skif update` on every instance it can move. It is not a lint rule and gets no rule ID: a rule would make linting depend on the network and on the day it runs. ADR 0009's table entry becomes "none: reported by `skif update`".

### `--interactive`

Deferred. A terminal picker is a CLI feature on top of the report, as ADR 0003 places interactive choices; `--dependencies` and `--target` cover scripted selection. `skif migrate` notes a script that uses it.

### Consequences

* Good, because a syncpack user drops syncpack for `update --check` in CI, and `skif migrate` translates `updateGroups`, `minimumReleaseAge` and `maxConcurrentRequests` exactly.
* Good, because linting keeps its guarantees: no connection, no clock, the same result on every machine.
* Good, because the planner is pure and tested with an in-memory registry, and the CLI is tested against a registry served on 127.0.0.1, so no test needs the internet.
* Good, because private registries work through `.npmrc`, which syncpack does not support.
* Bad, because skif now ships an HTTP and TLS stack (ureq, rustls, ring) in the CLI, with a duplicate `windows-sys` under a `deny.toml` skip.
* Bad, because the `.npmrc` subset is partial: basic auth, client certificates and `cafile` users get a failure, not an update.
* Bad, because the default scope differs from syncpack's (peer and optional dependencies stay put unless named), which a migrated project may notice.

### Confirmation

* `crates/skif-rules/src/update_tests.rs` covers targets, update groups, the release age and its pnpm fallback, semver groups, catalog entries, local packages, `catalog:` siblings, failures and one request per name, with a fake registry.
* `crates/skif/tests/update.rs` plans and applies updates through the facade on an in-memory file system.
* `crates/skif-cli/tests/update.rs` runs the binary against a registry on 127.0.0.1: `--check` exits `1` and writes nothing, the token is sent and never printed, writing keeps the formatting, a 404 and an unreachable registry exit `2`, writing is refused in CI.
* The CLI reference test fails when a flag of `skif update` is missing from `reference/cli.mdx`.

## Pros and Cons of the Options

### An opt-in `skif update` subcommand: the library plans through a `Registry` trait, the CLI does HTTP

* Good, because the network is one subcommand and one trait, and the offline guarantee holds everywhere else.
* Good, because hosts reuse the planner with their own client.
* Bad, because skif owns registry configuration handling that npm and pnpm already implement.

### Keep updates out of scope and document `syncpack update` alongside skif

* Good, because skif stays free of network code.
* Bad, because the second Node tool stays in CI for one command, which is what the issue reports.

### A lint rule that reports `DiffersToNpmRegistry`, behind a flag that enables the network

* Good, because the state would appear in the normal report and SARIF.
* Bad, because a lint result would depend on the registry and the date, and a flag that changes whether linting is offline is easy to leave on in CI.

### Shell out to `npm view` / `pnpm outdated`

* Good, because the package manager's own registry, auth and certificate handling would apply.
* Bad, because it needs Node and the package manager installed, starts one process per package or parses another tool's output, and cannot run in the library.
