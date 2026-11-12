---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# `skif update`: Opt-In Registry Updates, the Only Code That Uses the Network

## Context and Problem Statement

syncpack's `update` command, with `--check`, `--target`, `--interactive`, `updateGroups` and `minimumReleaseAge`,
moves dependencies to newer published versions. A team that replaces syncpack with skif would otherwise keep syncpack
installed for `syncpack update --check` in CI, the second Node install skif exists to remove. Updating needs a package
registry, while linting must stay offline and deterministic ([ADR 0001](0001-mission-scope-and-non-goals.md)).

Does skif talk to a registry, where does that code live, and how does it decide what moves and how far?

## Decision Drivers

* Linting stays offline, deterministic and safe on untrusted pull requests
* The library does no I/O ([ADR 0003](0003-library-api-and-embedding.md)): a host plans updates through its own
  client, a cache, or not at all
* syncpack's semantics for which instances move and how far, so `skif migrate` translates `updateGroups` and
  `minimumReleaseAge` one to one
* Private registries work, and credentials never reach a log, a report or the wrong host
* Every exit path is testable without a real registry
* A small dependency footprint that passes `deny.toml` and builds without a C toolchain
* A script or CI run never waits for input, and cancelling writes nothing

## Considered Options

* An opt-in `skif update` subcommand: the library plans through a `Registry` trait, the CLI does HTTP and the picker
* Keep updates out of scope and document `syncpack update` alongside skif
* A lint rule that reports `DiffersToNpmRegistry`, behind a flag that enables the network
* Shell out to `npm view` or `pnpm outdated`

## Decision Outcome

Chosen option: "An opt-in `skif update` subcommand: the library plans through a `Registry` trait, the CLI does HTTP
and the picker", because it gives syncpack's behaviour to users who ask for it while every other command keeps the
offline guarantee, and the network sits behind one trait that tests and hosts replace.

### Network position and the library split

* `skif update` is the only code in skif that opens a connection. Linting, `--fix`, `migrate`, `config`,
  `explain-usage` and the library never do; the library has no HTTP client.
* The planner is `skif::update` (facade feature `update`, which implies `manifest` and `fix`): a `Registry` trait,
  `Packument` (read from a registry document, so a host that does its own HTTP only fetches text),
  `plan_updates(workspace, config, registry, options)` with the clock passed in, and `apply_updates`, which returns a
  `FixPlan` through the fix planner ([ADR 0009](0009-fixes-edits-and-package-json-formatting.md)), so writes are
  format-preserving.
* The HTTP client, `.npmrc` reading, the clock and the picker live in `skif-cli`, the only crate that reads the
  environment.
* `DiffersToNpmRegistry` is a state `skif update` reports on every instance it can move. It is not a lint rule and
  has no rule id: a rule would make linting depend on the network and the date.

### Requests, registries and credentials

* One `GET <registry><name>` per distinct name, for the full packument (the abbreviated one has no publish times).
  Only a valid npm package name goes into a URL; any other name gets a note, not a failure. Nothing else is sent.
* Registry order: `npm_config_registry`, then `registry` in the project's `.npmrc`, then the user's `.npmrc`, then
  `https://registry.npmjs.org/`. `@scope:registry` chooses per scope, and `@jsr/*` goes to `npm.jsr.io` unless
  configured.
* Only `_authToken` entries are read, sent as `Authorization: Bearer` to the registry with the longest matching
  `//host/path/` prefix, and never printed. A redirect never carries a token. A token sent to an `http://` registry
  is sent with a warning, once per registry.
* The project's `.npmrc` arrives with the repository, so a pull request must not be able to point a CI secret at its
  own server: in it, `${VAR}` is expanded only in `_authToken`, and only when the token's host is `registry.npmjs.org`
  or a host the user's `.npmrc` or `npm_config_registry` names, and the project does not set an `http://` registry for
  that host. Any other line of it that reads a variable is skipped with a warning that names the key or host, never
  the value. The user's `.npmrc` and `npm_config_registry` keep full expansion.
* `_auth`, username and password, client certificates, `cafile` and `strict-ssl` are out of scope; a registry proxy
  covers them.
* Transport: `ureq` (blocking, no async runtime) over rustls with the ring provider and the operating system's trust
  store, proxy variables honoured, gzip; fixed connect, request and body-size limits; at most
  `[update] max-concurrent-requests` requests at once (syncpack's default).
* A package the registry cannot answer for is a failure: its instances are left alone, the others are still planned,
  and the run exits 2 because it is incomplete.

### What moves and how far

* Only instances in version groups that prefer the highest version, the catch-all included. Not workspace packages,
  a package's own version, siblings of a `catalog:` instance (the catalog entry moves instead) or instances the
  catalog rules own. Only specifiers syncpack updates, and only the dependency types `[policy] dependency-types`
  names. Catalog entries move where they are declared.
* The target is the highest published version that is not deprecated, has the same release channel, is an eligible
  update under the effective target, and is older than the minimum release age. The current range style is kept
  unless a semver group asks for another.
* `--target latest|minor|patch` (default `latest`). `[[update-groups]]` take the selectors of `[[semver-groups]]` and
  exactly one of `target` or `ignored = true`; the first match wins, and the stricter of the flag and the group's
  target applies.
* `[update] minimum-release-age` is in minutes, `0` turns it off, and unset it is pnpm's `minimumReleaseAge`, else one
  day, as in syncpack. While it applies, a version without a publish time is not eligible.
* `[update]` and `[[update-groups]]` are root-scoped: an override entry or a package file cannot set them.

### Checking, writing and the picker

* `--check` writes nothing and exits 1 when an update exists, 0 when none does, 2 when a package could not be checked.
* Otherwise updates are written atomically per file, then the package manager's install runs as after `--fix`
  ([ADR 0024](0024-cli-contract.md)) unless `--no-install`. Writing is refused when `CI` is truthy, as `--fix` is.
* `--format json` prints one document with each update's place, current and target specifier and state, the failures
  and the notes.
* `--interactive` plans as usual, then shows an `inquire` multi-select with one line per dependency and nothing
  preselected; the chosen lines' updates are kept in a filtered copy of the report, which `apply_updates` writes.
  Esc or Ctrl-C writes nothing and exits 0, and JSON adds a `declined` count. It needs a terminal on stdin and stdout,
  else it exits 2 with error kind `usage` before any request or write, and it conflicts with `--check`.

### Consequences

* Good, because a syncpack user drops syncpack for `update --check` in CI, and `skif migrate` translates
  `updateGroups`, `minimumReleaseAge` and `maxConcurrentRequests` exactly.
* Good, because linting keeps its guarantees: no connection, no clock, the same result on every machine.
* Good, because the planner is pure and tested with an in-memory registry, and the CLI against a registry on
  127.0.0.1, so no test needs the internet.
* Good, because private registries work through `.npmrc`, and hosts of the library are unaffected by the picker.
* Bad, because the CLI ships an HTTP and TLS stack, with a duplicate `windows-sys` under a `deny.toml` skip.
* Bad, because the `.npmrc` subset is partial: basic auth, client certificates and `cafile` users get a failure.
* Bad, because the default scope differs from syncpack's: peer and optional dependencies stay put unless named.
* Bad, because the picker cannot be scripted; `--dependencies` and `--target` are the scripted way.

### Confirmation

* `crates/skif-rules/src/update_tests.rs`: targets, update groups, the release age and its pnpm fallback, versions
  without a publish time, semver groups, catalog entries, local packages, `catalog:` siblings, failures and one
  request per name, with a fake registry.
* `crates/skif/tests/update.rs`: planning and applying updates through the facade on an in-memory file system.
* `crates/skif-cli/tests/update.rs`: `check_lists_the_updates_exits_one_and_writes_nothing`,
  `update_writes_the_new_versions_and_keeps_the_formatting`,
  `a_package_the_registry_does_not_know_makes_the_run_incomplete`,
  `an_unreachable_registry_exits_two_and_writes_nothing`, `writing_is_refused_when_ci_is_truthy_but_check_runs`,
  `a_project_token_from_the_environment_is_not_sent_to_an_http_registry_the_project_sets`,
  `a_project_token_from_the_environment_is_not_sent_to_a_host_the_user_did_not_configure`,
  `a_redirect_to_another_host_does_not_carry_the_token`, `a_registry_document_over_64_mib_is_refused`,
  `interactive_is_refused_without_a_terminal_before_any_request` and `interactive_and_check_conflict`.
* `crates/skif-cli/src/update.rs`: `only_the_chosen_lines_are_applied` and
  `choosing_nothing_or_an_unknown_line_applies_nothing`.
* `crates/skif-cli/src/args.rs`: `every_long_flag_is_in_the_cli_reference`.

## Pros and Cons of the Options

### An opt-in `skif update` subcommand: the library plans through a `Registry` trait, the CLI does HTTP and the picker

* Good, because the network is one subcommand and one trait, and the offline guarantee holds everywhere else.
* Good, because hosts reuse the planner with their own client, and the selection is a filter over a report they
  already get.
* Bad, because skif owns registry configuration handling that npm and pnpm already implement.

### Keep updates out of scope and document `syncpack update` alongside skif

* Good, because skif stays free of network code.
* Bad, because a second Node tool stays in CI for one command.

### A lint rule that reports `DiffersToNpmRegistry`, behind a flag that enables the network

* Good, because the state would appear in the normal report and SARIF.
* Bad, because a lint result would depend on the registry and the date, and a flag that makes linting go online is
  easy to leave on in CI.

### Shell out to `npm view` or `pnpm outdated`

* Good, because the package manager's own registry, auth and certificate handling would apply.
* Bad, because it needs Node and the package manager installed, starts one process per package or parses another
  tool's output, and cannot run in the library.
