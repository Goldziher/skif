# Architecture decision records

These records capture the design decisions behind skif, a Rust dependency linter for JavaScript and
TypeScript monorepos, in adrlint's MADR 4 format. Each record states the context, the options
considered, the decision, its consequences and how compliance is confirmed. Accepted records are
immutable and locked by `adrlint.lock`; a change of direction gets a new record that supersedes the
old one.

| ADR | Title | Status |
| --- | --- | --- |
| 0001 | [Mission, Scope and Non-Goals](0001-mission-scope-and-non-goals.md) | accepted |
| 0002 | [Crates, Library API and Upstream Code](0002-crates-library-api-and-upstream-code.md) | accepted |
| 0003 | [Configuration, Suppressions, Baselines and Migration](0003-configuration-suppressions-baselines-and-migration.md) | accepted |
| 0004 | [Workspaces, Roots and Package Managers](0004-workspaces-roots-and-package-managers.md) | accepted |
| 0005 | [Rules, Diagnostics and Fixes](0005-rules-diagnostics-and-fixes.md) | accepted |
| 0006 | [Version Policies, Lockfiles, Toolchain Files and Updates](0006-version-policies-lockfiles-toolchains-and-updates.md) | accepted |
| 0007 | [JS Analysis Core on oxc](0007-js-analysis-core-on-oxc.md) | accepted |
| 0008 | [Plugins, Config Evaluation and Commands](0008-plugins-config-evaluation-and-commands.md) | accepted |
| 0009 | [What Counts as a Use](0009-what-counts-as-a-use.md) | accepted |
| 0010 | [No Type-Aware Analysis](0010-type-aware-analysis-on-tsrs.md) | accepted |
| 0011 | [Nx, Lerna and Bazel](0011-nx-lerna-and-bazel.md) | accepted |
| 0012 | [CLI Contract and Reporting](0012-cli-contract-and-reporting.md) | accepted |
| 0013 | [Editors, Agents and Git Hooks](0013-editors-agents-and-git-hooks.md) | accepted |
| 0014 | [Benchmarks and the Comparison Harness](0014-benchmarks-and-the-comparison-harness.md) | accepted |
| 0015 | [Distribution, Release and Supported Versions](0015-distribution-release-and-supported-versions.md) | accepted |
