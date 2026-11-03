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
| 0016 | [CLI Contract: Flags, Exit Codes and Machine-Readable Errors](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) | accepted |
| 0017 | [Release Policy: A Single Complete 0.1.0](0017-release-policy-a-single-complete-0-1-0.md) | accepted |
| 0018 | [npm Distribution via napi-rs](0018-npm-distribution-via-napi-rs.md) | accepted |
| 0019 | [Reality Reconciliation: Crate List, Schema File, Test Tooling and Consistency Checks](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md) | accepted |
| 0020 | [JS Rule Set, Default Severities, Addon Profile and Windows Support](0020-js-rule-set-severities-addon-profile-and-windows-support.md) | accepted |
| 0021 | [oxc Lockstep With poly: One Release, Coordinated Bumps, a CI Build Against the Git Copy](0021-oxc-lockstep-with-poly.md) | accepted |
| 0022 | [Multi-Root Analysis, Single-Package Mode and the Plugin Set](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) | accepted |

## Effective statements

Several accepted records describe a staged release or name artefacts that changed later. Records are locked, so
the lines below stay as written; this table says which record governs.

| Record and line | Statement | Effective statement |
| --- | --- | --- |
| [0001](0001-mission-and-scope.md), line 89 | 0.1.0 ships manifest rules, policies and distribution; 0.2.0 adds JS analysis | [0017](0017-release-policy-a-single-complete-0-1-0.md): one complete 0.1.0 contains JS analysis and the plugin set |
| [0001](0001-mission-and-scope.md), line 122 | the first release deliberately ships two of the three parts | [0017](0017-release-policy-a-single-complete-0-1-0.md): all three parts ship in 0.1.0 |
| [0004](0004-configuration-model.md), line 105 | `[js] enabled` is harmless in 0.1.0 because the feature is off | [0017](0017-release-policy-a-single-complete-0-1-0.md): JS analysis is in 0.1.0 and runs by default |
| [0010](0010-js-analysis-on-oxc.md), line 116 | the plugin set ships in 0.2.0 | [0017](0017-release-policy-a-single-complete-0-1-0.md): it ships in 0.1.0 |
| [0010](0010-js-analysis-on-oxc.md), lines 89-91 | fifteen issue types map to fifteen rules | [0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md): thirteen rules; the two namespace rules are withdrawn |
| [0010](0010-js-analysis-on-oxc.md), consequences | exact oxc pins conflict with a host on a different oxc; such a host leaves `js-analysis` off or aligns its pin | [0021](0021-oxc-lockstep-with-poly.md): skif tracks the oxc release poly pins, so poly patches skif's oxc crates to its git copy |
| [0011](0011-plugin-model.md), line 88 | the first 21 plugins ship in 0.2.0 | [0017](0017-release-policy-a-single-complete-0-1-0.md): they ship in 0.1.0 |
| [0011](0011-plugin-model.md), line 88 | the first set is 21 plugins | [0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md): the table holds 25 plugins |
| [0012](0012-reporting.md), JSON document | one summary, no per-root data | [0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md): a multi-root run adds a `roots` array |
| [0013](0013-distribution-and-naming.md), lines 53, 76 and 86 | the floating Action tag is `v1` | [0017](0017-release-policy-a-single-complete-0-1-0.md): `v0` while the version is 0.x |
| [0013](0013-distribution-and-naming.md), npm channel | a launcher spawns a binary from eight platform packages | [0018](0018-npm-distribution-via-napi-rs.md): a napi addon from six platform packages, no Windows npm packages |
| [0014](0014-git-hook-integration.md), line 69 | JS analysis is compiled in "from 0.2.0 on" | [0017](0017-release-policy-a-single-complete-0-1-0.md): from 0.1.0 on |
| [0018](0018-npm-distribution-via-napi-rs.md), addon profile | an unwinding profile, unnamed | [0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md): a dedicated `panic = "unwind"` profile |
| [0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md), crate list | twelve crates, `skif-js` pending | [0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md): fourteen crates including `skif-js` and the unpublished `skif-node` |
