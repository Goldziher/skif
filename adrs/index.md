# Architecture decision records

These records capture the design decisions behind skif, a Rust dependency linter for JavaScript and
TypeScript monorepos, in adrlint's MADR 4 format. Each record states the context, the options
considered, the decision, its consequences and how compliance is confirmed. Accepted records are
immutable and locked by `adrlint.lock`; a change of direction gets a new record that supersedes the
old one.

| ADR | Title | Status |
| --- | --- | --- |
| 0001 | [Mission, Scope and Non-Goals](0001-mission-scope-and-non-goals.md) | accepted |
| 0002 | [Crates, Publishing and Repository Consistency Checks](0002-crates-publishing-and-repository-consistency-checks.md) | accepted |
| 0003 | [Library API and Embedding](0003-library-api-and-embedding.md) | accepted |
| 0004 | [Configuration Model and Layering](0004-configuration-model-and-layering.md) | accepted |
| 0005 | [The `[js]` Table, Ignore Lists and Suppression Reasons](0005-the-js-table-ignore-lists-and-suppression-reasons.md) | accepted |
| 0006 | [Workspace Discovery, Roots and Multi-Root Runs](0006-workspace-discovery-roots-and-multi-root-runs.md) | accepted |
| 0007 | [Specifier and Semver Model](0007-specifier-and-semver-model.md) | accepted |
| 0008 | [Rule and Diagnostic Model, Rule Ids and Severities](0008-rule-and-diagnostic-model-rule-ids-and-severities.md) | accepted |
| 0009 | [Fixes, Edits and `package.json` Formatting: Structural Operations, Returned Plans and Safety by Move](0009-fixes-edits-and-package-json-formatting.md) | accepted |
| 0010 | [Version, Semver-Range and Catalog Policies: syncpack Semantics on a Few Stable Rule IDs](0010-version-semver-range-and-catalog-policies.md) | accepted |
| 0011 | [Lockfile and Toolchain Rules: Lockfile Scopes, Stray Lockfiles, Patches and Pins Outside `package.json`](0011-lockfile-and-toolchain-rules.md) | accepted |
| 0012 | [Package-Manager Semantics: pnpm, Yarn, Bun, npm and Rush](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md) | accepted |
| 0013 | [JS Analysis Core: knip Semantics on oxc, the Module Graph and Resolution](0013-js-analysis-core-oxc-module-graph-and-resolution.md) | accepted |
| 0014 | [Files the JS Analysis Reads: The Walk, the Git Index and Fixture Directories](0014-files-the-js-analysis-reads-walk-git-index-fixtures.md) | accepted |
| 0015 | [Plugin Model and Plugin Set: A Declarative Table With Optional Static Resolvers](0015-plugin-model-and-plugin-set.md) | accepted |
| 0016 | [Config Evaluation and Incomplete Entries: A Sound Static Evaluator and Narrowed Unjudged Scopes](0016-config-evaluation-and-incomplete-entries.md) | accepted |
| 0017 | [Scripts, Binaries and Working Directories](0017-scripts-binaries-and-working-directories.md) | accepted |
| 0018 | [Export, Namespace and Member Rules](0018-export-namespace-and-member-rules.md) | accepted |
| 0019 | [Dependency, Unlisted and Unresolved Findings](0019-dependency-unlisted-and-unresolved-findings.md) | accepted |
| 0020 | [Templates, Components and Custom Elements](0020-templates-components-and-custom-elements.md) | accepted |
| 0021 | [knip and syncpack Comparison Harness](0021-knip-and-syncpack-comparison-harness.md) | accepted |
| 0022 | [Reporting and Output Formats](0022-reporting-and-output-formats.md) | accepted |
| 0023 | [Baselines](0023-baselines.md) | accepted |
| 0024 | [CLI Contract](0024-cli-contract.md) | accepted |
| 0025 | [`skif migrate`: A One-Shot Translator From sherif, syncpack and knip](0025-skif-migrate.md) | accepted |
| 0026 | [`skif update`: Opt-In Registry Updates, the Only Code That Uses the Network](0026-skif-update.md) | accepted |
| 0027 | [Distribution and Release: One Name, One Version, One Release Workflow](0027-distribution-and-release.md) | accepted |
| 0028 | [Editors, Agents and Git Hooks: `skif mcp`, `skif lsp`, `--watch`, the Agent Skill and the Hooks](0028-editors-agents-and-git-hooks.md) | accepted |
| 0029 | [Performance, Caching and Benchmarks: A Content-Keyed Module Cache and Published Numbers From a Pinned Runner](0029-performance-caching-and-benchmarks.md) | accepted |
| 0030 | [Supported Versions and the End-to-End Matrix: Upstream End of Life, Tested Layouts](0030-supported-versions-and-the-end-to-end-matrix.md) | accepted |
| 0031 | [Bazel: Static Reading of Starlark, Output Trees That Are Never Read, and Paths Under `bazel run` and `bazel test`](0031-bazel.md) | accepted |
| 0032 | [Nx and Lerna (and Pruned Output Trees): Projects as Scopes, Lerna as Facts, Layouts Recognised by Shape](0032-nx-and-lerna-and-pruned-output-trees.md) | accepted |
| 0033 | [Attribution and Vendoring: Depend, Vendor, Re-Implement or Reference, and One Attributions File](0033-attribution-and-vendoring.md) | accepted |
