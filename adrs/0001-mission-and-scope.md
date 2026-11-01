---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Mission and Scope: One Fast Dependency Linter for JavaScript Monorepos

## Context and Problem Statement

A JavaScript monorepo needs three kinds of dependency hygiene, and today each comes from a
different tool with a different runtime, configuration format and speed profile:

1. **Workspace and manifest rules** — every package has a `package.json`, workspace patterns
   match something, the root is `private`, dependencies are sorted, `@types/*` sit in
   `devDependencies`. sherif covers this, in Rust, with a fixed rule set and almost no
   configuration.
2. **Version policy** — one version of `react` across the repo, pinned ranges for some
   dependencies, `workspace:` for internal packages, pnpm catalogs used where they exist.
   syncpack covers this with version groups and semver groups, as a Node CLI.
3. **Usage analysis** — dependencies listed but never imported, imports of packages that are not
   listed, unused files and exports. knip covers this, as a Node CLI that loads every tool's
   configuration by executing it.

This repository started as a fork of sherif. Its code is a single binary crate (about 5,600 lines
under `src/`) whose rules print, prompt (`inquire::Select` inside `multiple_dependency_versions`)
and write files themselves, and whose fixes write to paths relative to the process working
directory (`PathBuf::from("package.json")` in `root_package_private_field`), so `--fix` run from
anywhere but the workspace root edits the wrong file or fails. None of that can be embedded, and
none of it extends to the second and third kinds of check.

## Decision Drivers

* One run, one configuration file and one diagnostic model for workspace rules, version policy and usage analysis
* An embeddable library, not a binary whose rules print, prompt and exit the process
* Performance targets measured by `mise run bench`
* Safe to run on untrusted pull requests, so no execution of user JavaScript

## Considered Options

* One Rust dependency linter covering manifest rules, version policy and JS usage analysis
* Keep shipping sherif and add features to it
* Wrap syncpack and knip (spawn them and merge their output)
* Three separate Rust tools (one per kind of check)

## Decision Outcome

Chosen option: "One Rust dependency linter covering manifest rules, version policy and JS usage analysis", because it replaces three tools, three runtimes and three configuration formats with one library-first tool, one configuration file and one diagnostic model, and it refuses to execute user JavaScript.

skif is a **Rust dependency linter for JavaScript and TypeScript monorepos**, built as a library
first ([ADR 0003](0003-library-first-embedding.md)) with a thin CLI, that covers all three kinds of check in one run, one
configuration file ([ADR 0004](0004-configuration-model.md)) and one diagnostic model ([ADR 0007](0007-rule-and-diagnostic-model.md)):

* **Workspace and manifest rules** in the spirit of sherif, re-implemented on the new model
  ([ADR 0005](0005-workspace-discovery.md), [ADR 0007](0007-rule-and-diagnostic-model.md)).
* **syncpack-style policies**: version groups, semver groups, catalogs, the workspace protocol and
  dependency families ([ADR 0006](0006-specifier-and-semver-model.md), [ADR 0009](0009-version-and-catalog-policies.md)).
* **knip-style analysis** of unused and unlisted dependencies, unresolved imports, unused files,
  exports, types and members, on oxc ([ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md)).

It is a **clean rebuild, not a sherif release**. Nothing is inherited by default: no `sherif` key
in `package.json`, no sherif rule IDs where a clearer one exists ([ADR 0007](0007-rule-and-diagnostic-model.md)), no command-line
compatibility. Code is ported only where it is still the best implementation, and credited
([ADR 0015](0015-attribution-and-vendoring.md)).

Explicitly **out** of scope:

* **Registry updates and an interactive upgrade TUI** (syncpack's `update`). skif reads the
  workspace and never contacts a package registry.
* **Formatting `package.json`** beyond the edits a fix makes. Key order outside dependency maps,
  indentation and whitespace belong to a formatter; fixes preserve them ([ADR 0008](0008-fix-and-edit-model.md)).
* **Lockfile analysis.** skif reasons about declared specifiers, not resolved trees.
* **Executing user JavaScript.** Tool configuration such as `vite.config.ts` is read by static AST
  extraction only ([ADR 0010](0010-js-analysis-on-oxc.md)). skif never spawns Node, never `require`s a config, and never runs a
  package script.

Performance targets, measured by `mise run bench` (criterion plus hyperfine) on Apple silicon:

| Workload | Target |
| --- | --- |
| Manifest and policy rules, 500 packages | < 50 ms wall clock |
| JS analysis on the corpus repositories | ≥ 5× faster than `knip` on the same tree |
| Parse and build the module graph, 10,000 files | < 2 s |

Releases follow the work: **0.1.0** ships manifest rules, policies and distribution; **0.2.0** adds
JS analysis and the first plugin set ([ADR 0011](0011-plugin-model.md)).

### Consequences

* Good, because one tool, one config and one report replace three tools with three runtimes. A monorepo that
  already runs sherif, syncpack and knip in CI drops two Node installs and two configuration
  formats.
* Good, because library-first means a host such as poly gets the same rules as the CLI, with no subprocess and
  no output parsing.
* Good, because refusing to execute JavaScript makes skif safe to run on untrusted pull requests and
  deterministic across machines, which knip's config loading is not.
* Bad, because re-implementing syncpack and knip semantics means tracking two moving upstreams. Each ADR that
  takes one as a specification names the upstream commit it was read at, and the corpus diff
  (`mise run parity`, `mise run diff:syncpack`) measures drift rather than assuming parity.
* Bad, because static config extraction cannot see values computed at runtime; some configurations that knip
  understands will be partially understood by skif ([ADR 0010](0010-js-analysis-on-oxc.md)).
* Bad, because breaking sherif compatibility costs existing sherif users a migration. That is accepted:
  skif is a different tool under a different name ([ADR 0013](0013-distribution-and-naming.md)), not sherif's successor release.

### Confirmation

* `mise run bench` (criterion plus hyperfine) measures the performance targets in the table above.
* `mise run parity` and `mise run diff:syncpack` compare skif with knip and syncpack on the corpus and measure drift instead of assuming parity.

## Pros and Cons of the Options

### One Rust dependency linter covering manifest rules, version policy and JS usage analysis

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Keep shipping sherif and add features to it

* Bad, because the rules own IO, prompts and process exit, so every new capability would inherit the cwd bug class and the lack of an embeddable API. Fixing that is the rebuild.

### Wrap syncpack and knip (spawn them and merge their output)

* Bad, because it requires Node, pays two process start-ups and knip's config execution on every run, and cannot meet the performance targets or be embedded.

### Three separate Rust tools (one per kind of check)

* Bad, because the checks share workspace discovery, specifier parsing, configuration, diagnostics and fixes; splitting them would duplicate all of it and give the user three configs again.

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md), [ADR 0013](0013-distribution-and-naming.md), [ADR 0015](0015-attribution-and-vendoring.md)
* Upstream: [sherif](https://github.com/QuiiBz/sherif)
* Upstream: [syncpack](https://github.com/JamieMason/syncpack)
* Upstream: [knip](https://github.com/webpro-nl/knip)
* See the [ADR index](index.md).
