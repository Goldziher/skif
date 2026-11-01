# Architecture decision records

These records capture the design decisions behind skif, a Rust dependency linter for JavaScript and
TypeScript monorepos, in adrlint's MADR 4 format. Each record states the context, the options
considered, the decision, its consequences and how compliance is confirmed. Accepted records are
immutable and locked by `adrlint.lock`; a change of direction gets a new record that supersedes the
old one.

| ADR | Title | Status |
| --- | --- | --- |
| 0001 | [Mission and Scope: One Fast Dependency Linter for JavaScript Monorepos](0001-mission-and-scope.md) | accepted |
| 0002 | [Crate Layout and Publishing: Fourteen Crates, One Stable Facade](0002-crate-layout-and-publishing.md) | accepted |
| 0003 | [Library-First Embedding: The CLI Is One Consumer of the Library](0003-library-first-embedding.md) | accepted |
| 0004 | [Configuration Model: One TOML File, Typed Options, Defaults < File < CLI](0004-configuration-model.md) | accepted |
| 0005 | [Workspace Discovery: Facts From Four Package Managers, Decisions in Rules](0005-workspace-discovery.md) | accepted |
| 0006 | [Specifier and Semver Model: Vendored syncpack-specifier on nodejs-semver](0006-specifier-and-semver-model.md) | accepted |
| 0007 | [Rule and Diagnostic Model: Stable IDs, Providers, One Emitter](0007-rule-and-diagnostic-model.md) | accepted |
| 0008 | [Fix and Edit Model: Structural Operations, Format-Preserving Edits, Returned Plans](0008-fix-and-edit-model.md) | accepted |
| 0009 | [Version and Catalog Policies: syncpack Semantics, Few Stable Rule IDs](0009-version-and-catalog-policies.md) | accepted |
| 0010 | [JS Analysis on oxc: knip Semantics, Static Configs, Three Crates](0010-js-analysis-on-oxc.md) | accepted |
| 0011 | [Plugin Model: A Declarative Table With Optional AST Resolvers](0011-plugin-model.md) | accepted |
| 0012 | [Reporting: Four Renderers, Stable JSON, Three Exit Codes](0012-reporting.md) | accepted |
| 0013 | [Distribution and Naming: skif, Eight Targets, One Version Everywhere](0013-distribution-and-naming.md) | accepted |
| 0014 | [Git Hook Integration: pre-commit Launcher, poly Catalog, Lefthook Docs](0014-git-hook-integration.md) | accepted |
| 0015 | [Attribution and Vendoring: Vendor, Re-Implement or Depend, and Credit Every Source](0015-attribution-and-vendoring.md) | accepted |
