---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# Version Policies, Lockfiles, Toolchain Files and Updates

## Context and Problem Statement

Every version policy compares dependency specifiers, and a `package.json` specifier is not a Cargo requirement: it can
be an npm range with `||` or a hyphen, a dist-tag, `workspace:`, `catalog:`, an `npm:` alias, a path or a URL.
syncpack defines the policy model its users know: version groups that partition dependency instances and give each a
policy, semver groups that decide the range type, and a state per instance that says why it is valid or not. Catalogs,
the `workspace:` protocol and override sections hold versions too. Beyond manifests, the lockfile says what a workspace
installs, and Dockerfiles, CI workflows, version files and shell scripts say which Node and which package manager
build it. Moving dependencies to newer published versions needs a registry, while linting must stay offline.

How does skif parse specifiers, express version and catalog policies, check lockfiles and toolchain files, and update
dependencies from a registry?

## Decision Drivers

* npm range semantics and syncpack's classification, so a syncpack user finds the model they know
* A workspace without configuration still gets sherif's default version check
* A few stable rule IDs keep configuration usable, while the exact state stays available to tooling
* A check that cannot run says so, and a deliberate exception carries a reason
* Linting stays offline and deterministic; only an explicit command talks to a registry

## Considered Options

* syncpack's model on a vendored specifier parser, few rule IDs with the state in `detail`, on-demand lockfile and
  toolchain rules, and an opt-in `skif update` behind a `Registry` trait
* One rule ID per syncpack state
* Call syncpack, or keep sherif's two hard-coded checks, which cannot be scoped, pinned or relaxed
* Leave updates to `syncpack update`, which keeps a second Node install in CI

## Decision Outcome

Chosen option: "syncpack's model on a vendored specifier parser, few rule IDs with the state in `detail`, on-demand
lockfile and toolchain rules, and an opt-in `skif update` behind a `Registry` trait", because syncpack users get the
semantics they know, sherif's check survives as a configurable catch-all, and the network stays behind one command.

### Specifiers

* `skif-specifier` is syncpack's specifier model, vendored and made thread-safe on `nodejs-semver`
  ([ADR 0002](0002-crates-library-api-and-upstream-code.md) records how it is vendored). It is the only place where
  specifiers are parsed; discovery parses each manifest entry once and shares the result with every rule. The `semver`
  crate is never used for npm specifiers.

### Policies

* `skif-rules/policy` re-implements syncpack's semantics on skif's own instance model. syncpack is the specification,
  not a dependency. Selector matching is the compiled `Selector` of `skif-config`, shared with `[[ignore]]` and the
  ignore lists ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)).
* Every state syncpack can assign maps to exactly one rule or to none, and the state name is the diagnostic's `detail`
  ([ADR 0005](0005-rules-diagnostics-and-fixes.md)). The mapping lives with the code and is checked by a test. skif
  adds `UnusedGroup`, `InvalidPin`, `DiffersToOtherCatalog` and `IsIdenticalToCatalog`. `DiffersToNpmRegistry` is no
  rule; `skif update` reports it.
* Deliberate differences from syncpack: a range the local version satisfies is valid next to it; `latest` and `*` are
  never the alignment target; ranges rank by the lowest version they admit; an `npm:` alias is a dependency of its own;
  packages are read with the package manager's workspace semantics
  ([ADR 0004](0004-workspaces-roots-and-package-managers.md)). The comparison harness classifies these
  ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).
* Default scope: `dependencies`, `devDependencies` and each package's own version (`local`). Peer and optional ranges
  state a compatibility window, so aligning them is opt-in, as are override sections and `[[custom-types]]`;
  `[policy] dependency-types` widens or narrows the scope. `[policy] override-reading` decides the dependency of an
  override entry: `skif` (default) takes the package its selector replaces, `syncpack` takes the key as written and
  skips nested entries; `skif migrate` sets it.
* `[[version-groups]]` are evaluated in file order and the first match wins. After them a catch-all
  `preferred-semver` group takes every remaining instance in scope and reports `version-mismatch`: sherif's check,
  expressed as a policy. A configured group that matches nothing is `policy-conflict` with `detail` `UnusedGroup`.
  Policies that require incompatible outcomes for one instance give one `policy-conflict` without a fix, since no
  edit satisfies both.
* The first matching configured group claims an instance whatever its policy, and the catalog rules do not see a
  claimed instance, so each instance has one owner. Path specifiers are never rewritten and never an alignment target;
  while `local-path-dependency` is on, it owns them. Semver groups claim nothing.
* `[[semver-groups]]` decide the range type (`semver-range-mismatch`), `workspace-protocol` requires `workspace:` for
  internal dependencies where the manager installs it, and `[[dependency-families]]` are packages released together
  that must share a version (`unsynced-dependency-family`).
* Catalogs: `[catalog] mode` is `off` (default), `prefer` or `enforce`. Each catalog source is read only for the
  manager that uses it, so a fix never writes into a file the active manager does not read.
* `skif list` and `skif json` show every governed instance with its group and state from the same analysis as the
  rules, so the listing cannot drift from them. The `--dependency`-style filters drop instances before groups are
  assigned and reach only the version-group and semver-group rules and the listings; no configuration file can set a
  filter.

### Lockfiles

* `skif-workspace` models `pnpm-lock.yaml` in lockfile format 9 (the project document of a two-document lockfile) and
  `bun.lock`. `bun.lockb`, `yarn.lock` and `package-lock.json` are not modelled.
* Discovery does not read the lockfile. Rules read it on demand through `LintContext::fs`, the file system the host
  passes, and share one parse per run, so a configuration that asks for no lockfile check reads no lockfile.
* A lockfile check that cannot run (no lockfile, another manager's, one that does not parse) reports why at its usual
  severity; it never passes silently.
* Lockfile scopes reuse the policy shapes: a `[[version-groups]]` entry with `scope = "lockfile"` and
  `policy = "banned"` reports each resolved `name@version` with the shortest chain that pulls it in, and a
  `[[dependency-families]]` entry with `scope = "importer"` or `"lockfile"` is judged on what the lockfile resolves.
* `stray-lockfile` reports a lockfile in a member directory and a root lockfile the detected manager does not read. A
  member lockfile a build installs from is not stray: one a Bazel label or `BUILD` file names
  ([ADR 0011](0011-nx-lerna-and-bazel.md)), or one a Dockerfile copies before an install. Finding them reads files as
  text and runs nothing. Its fix deletes the file and needs `--allow-remove-files`.
* `nested-workspace-root` reports a `pnpm-workspace.yaml` in a member directory, under Bun a member `package.json`
  that declares `workspaces`, and a member inside a Bazel convenience link; an npm or Yarn member with `workspaces` is a
  root of its own ([ADR 0004](0004-workspaces-roots-and-package-managers.md)). The file may hold settings that belong
  in the root, so there is no deleting fix.
* `stale-patched-dependencies` reports a patch the lockfile does not resolve or apply, a missing patch file, and a
  patch file no entry references.
* `allow` entries of these rules are strings or `{ name, reason }` tables, and an entry that matches nothing is
  reported as stale.

### Toolchain files

* One scan, shared by the toolchain rules, reads version files, Dockerfiles, shell scripts, the root's GitHub
  workflows and Bazel module files at the root and in every package directory. A file belongs to the innermost
  package that holds it. Files are read by line and commands are split by `skif_core::shell`, the tokenizer the
  `package.json` script parser shares, so these rules do not depend on oxc.
* `toolchain-pin-drift` compares each literal Node or package-manager pin with the root's source of truth (the first
  root version file, else `engines.node`; `packageManager`, else `devEngines.packageManager`). Both sides are
  node-semver ranges, and they drift when no version satisfies both. The repair differs per tool, so there is no fix.
* `foreign-package-manager-command` reports install, run and exec commands of a manager other than the detected one;
  without a detected manager nothing is reported.
* `# skif-ignore-next-line [rule, …] -- <reason>` and `# skif-ignore-file [rule, …] -- <reason>` suppress toolchain
  findings. A comment without a reason suppresses nothing, and the finding's help says so.
* Findings on lockfiles and toolchain files default to `warn`, so adopting them fails no build that does not pass
  `--fail-on-warnings`.

### `skif update`

* `skif update` is the only code in skif that opens a network connection
  ([ADR 0001](0001-mission-scope-and-non-goals.md)). Linting, `--fix`, `migrate` and the library never do.
* The planner is `skif::update` (facade feature `update`): a `Registry` trait, `Packument` parsed from a registry
  document, and a pure `plan_updates` with the clock passed in. Applying returns a `FixPlan`, so writes are
  format-preserving. The HTTP client, `.npmrc` reading, the clock and the interactive picker live in `skif-cli`.
* Credentials: only `_authToken` entries are read, sent to the registry with the longest matching prefix, never printed
  and never carried across a redirect. The project's `.npmrc` arrives with the repository, so in it `${VAR}` expands
  only in a token for `registry.npmjs.org` or a host the user's own configuration names; a pull request cannot point a
  CI secret at its own server. Other authentication schemes are out of scope.
* What moves follows syncpack: instances in groups that prefer the highest version, within `[policy] dependency-types`;
  catalog entries move where they are declared, never their `catalog:` references. The target is the highest
  published, non-deprecated version on the same release channel within `--target` and `[[update-groups]]`, older than
  the minimum release age (pnpm's `minimumReleaseAge`, else one day). The range style is kept. A dependency on a workspace
  package, a package's own version and an instance the catalog rules own never move, and `[update]` is root-scoped.
* `--check` writes nothing and exits 1 when an update exists. A package the registry cannot answer for leaves its
  instances alone and makes the run exit 2. Writing is refused in CI
  ([ADR 0012](0012-cli-contract-and-reporting.md)).

### Consequences

* Good, because syncpack users find the model they know, and the listing comes from the same analysis as the rules.
* Good, because a workspace with no configuration gets sherif's check, and one with groups gets it for what its groups
  do not claim.
* Good, because a transitive ban, a family split, an unapplied patch, a stray lockfile and a pin drift are reported
  where they are fixed, and a lockfile a build installs from is not.
* Good, because linting keeps its guarantees: no connection, no clock, the same result on every machine.
* Bad, because re-implementing a state machine from a specification means matching its edge cases one by one, and
  first match wins is order-sensitive.
* Bad, because several states share a rule ID, so silencing one silences its siblings unless `[[ignore]]` narrows it.
* Bad, because only pnpm format 9 and `bun.lock` are modelled, and toolchain files are read by line rather than
  parsed, so heredoc `RUN` bodies and YAML flow mappings are not understood.
* Bad, because the CLI ships an HTTP and TLS stack, and the `.npmrc` subset is partial.

### Confirmation

* `scripts/check-headers.sh` checks every vendored file under `crates/skif-specifier/src/` for its header.
* `crates/skif-specifier/src/hardening_test.rs`, `ordering_test.rs` and `eq_hash_test.rs` hold the panic-freedom and
  ordering laws.
* `crates/skif-rules/src/policy/groups.rs` and `visit.rs` hold group assignment and ownership;
  `crates/skif-rules/tests/policy_catalog_conformance.rs` and `crates/skif/tests/policy_conformance.rs` run the
  fixtures under `fixtures/policy`, and `mise run diff:syncpack` compares classification with syncpack.
* `crates/skif-workspace/src/lockfile.rs`, `crates/skif-workspace/src/bun_lock.rs` and
  `crates/skif-rules/src/policy/lockfile_bans.rs` cover the lockfile model and the check that cannot run.
* `crates/skif-rules/src/manifest/stray_lockfile.rs`, `stale_patched_dependencies.rs` and
  `crates/skif-rules/src/toolchain/` cover the lockfile and toolchain rules.
* `crates/skif-rules/src/update_tests.rs`, `crates/skif/tests/update.rs` and `crates/skif-cli/tests/update.rs` cover
  `skif update` against an in-memory registry and a registry on 127.0.0.1, including the credential rules.
