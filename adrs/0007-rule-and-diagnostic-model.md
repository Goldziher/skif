---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Rule and Diagnostic Model: Stable IDs, Providers, One Emitter

## Context and Problem Statement

Rule IDs are what users type: in `[rules]`, in `[[ignore]]`, in CI scripts that grep JSON output,
in SARIF baselines. Once published they cannot change without breaking every one of those.

The inherited IDs are sherif's, and several are wrong or unclear: `non-existant-packages` is
misspelled and describes a pattern, not a package; `unordered-dependencies` reports dependencies
that are not *sorted*; `multiple-dependency-versions` is one special case of a version policy.
Each inherited rule also applies its own severity and ignore logic, and an issue is a trait object
that knows how to print and fix itself (`rules/mod.rs`), so there is no uniform finding a host or
reporter can consume.

skif's rules come from three places — manifest rules, policies ([ADR 0009](0009-version-and-catalog-policies.md)) and JS analysis
([ADR 0010](0010-js-analysis-on-oxc.md)) — and the JS rules live in a crate that most builds do not compile ([ADR 0003](0003-library-first-embedding.md)). The
registry therefore cannot be a hard-coded list in one crate.

## Decision Drivers

* Rule IDs are typed in configuration, CI scripts and SARIF baselines and cannot change once published
* Several inherited sherif IDs are misspelled or unclear
* Each inherited rule applied its own severity and ignore logic, and no uniform finding existed
* JS rules live in a crate most builds do not compile, so the registry cannot be a hard-coded list

## Considered Options

* Frozen kebab-case rule IDs, a RuleProvider registry and a central Emitter
* Keep sherif IDs for continuity
* Accept old IDs as aliases
* Rules apply their own severity and ignores
* Numeric IDs (`SK001`)

## Decision Outcome

Chosen option: "Frozen kebab-case rule IDs, a RuleProvider registry and a central Emitter", because IDs are public API and must be self-describing and permanent, and a central emitter makes severity and suppression behave identically for every rule, including rules a host adds.

### Rule IDs are public API

Every rule has a **kebab-case ID that never changes** once released. A rule that changes meaning
gets a new ID; a retired ID stays reserved and is rejected with a pointer to its replacement.

Kept from sherif: `empty-dependencies`, `root-package-private-field`, `root-package-manager-field`,
`root-package-dependencies`, `types-in-dependencies`, `member-toolchain-fields`,
`incompatible-peer-dependencies`.

Renamed (the old ID is not accepted; skif is not sherif, [ADR 0001](0001-mission-and-scope.md)):

| sherif ID | skif ID |
| --- | --- |
| `unordered-dependencies` | `unsorted-dependencies` |
| `non-existant-packages` | `unmatched-workspace-patterns` |
| `packages-without-package-json` | `missing-package-manifest` |
| `local-path-dependencies` | `local-path-dependency` |
| `multiple-dependency-versions` | `version-mismatch` |
| `unsync-similar-dependencies` | `unsynced-dependency-family` |

New policy IDs ([ADR 0009](0009-version-and-catalog-policies.md)): `workspace-protocol`, `banned-dependency`, `semver-range-mismatch`,
`catalog-mismatch`, `catalog-not-used`, `catalog-missing-entry`, `unused-catalog-entries`,
`unresolved-catalog-references`, `policy-conflict`, `invalid-local-version`.

New JS IDs ([ADR 0010](0010-js-analysis-on-oxc.md)): `unused-dependencies`, `unused-dev-dependencies`, `unlisted-dependencies`,
`unresolved-imports`, `unlisted-binaries`, `unused-optional-peer-dependencies`, `unused-files`,
`unused-exports`, `unused-types`, `unused-namespace-exports`, `unused-namespace-types`,
`duplicate-exports`, `unused-enum-members`, `unused-namespace-members`, `circular-imports`.

IDs name the problem in the singular when each diagnostic is one instance (`local-path-dependency`)
and in the plural when they mirror an established upstream issue type that users already know
(`unused-dependencies`).

### Rule, provider, registry

```rust
pub trait Rule: Send + Sync {
    fn meta(&self) -> &RuleMeta;          // id, default severity, category, fixable, docs
    fn check(&self, cx: &LintContext<'_>, emit: &mut Emitter<'_>);
}

pub trait RuleProvider: Send + Sync {
    fn rules(&self) -> Vec<Box<dyn Rule>>;
}
```

`skif-rules` provides the manifest and policy rules; `skif-js` provides the JS rules. The facade
assembles a `Registry` from the providers its features enable. The registry rejects a duplicate ID
at construction and is the single source for `skif rules`, the JSON Schema's rule list ([ADR 0004](0004-configuration-model.md))
and the generated `docs/rules.md`.

`LintContext` carries the workspace facts ([ADR 0005](0005-workspace-discovery.md)), the typed configuration and the rule's own
typed options, and the specifier cache ([ADR 0006](0006-specifier-and-semver-model.md)). It is read-only.

### The emitter owns policy, rules own detection

A rule calls `emit.report(Diagnostic { .. })` for every problem it finds and does nothing else. The
`Emitter`, created per rule by `skif-lint`, applies centrally:

* the rule's configured severity (a rule set to `off` is not run at all);
* `[[ignore]]` entries matching the rule × package × dependency;
* the CLI and host overrides ([ADR 0004](0004-configuration-model.md)).

Rules run in parallel on rayon. Their diagnostics are collected and **sorted deterministically**
by path, span start, rule ID and message, so output is identical across runs and thread counts.

### Diagnostic shape

```rust
pub struct Diagnostic {
    pub rule: RuleId,
    pub severity: Severity,             // Error | Warning
    pub message: String,                // one line, no trailing period
    pub help: Option<String>,
    pub location: Location,             // { path, span: Option<Span> }
    pub package: Option<String>,
    pub dependency: Option<String>,
    pub detail: Option<Detail>,         // rule-specific structured data
    pub fixes: Vec<Fix>,                // ADR 0008
}

pub struct Span {
    pub start: usize, pub end: usize,   // byte offsets into the file
    pub start_line: u32, pub start_col: u32,
    pub end_line: u32, pub end_col: u32, // 1-based; columns count UTF-8 characters
}
```

Paths are workspace-root-relative with `/` separators. The line and column fields are the four
fields of poly's `Span`, and the byte range is the range poly's `Edit` replaces, so a host maps a
skif diagnostic without re-reading the file ([ADR 0003](0003-library-first-embedding.md)). Byte offsets serve edits ([ADR 0008](0008-fix-and-edit-model.md)); line
and column serve humans, SARIF and GitHub annotations ([ADR 0012](0012-reporting.md)). `detail` carries machine data such
as the exact syncpack instance state behind a policy diagnostic ([ADR 0009](0009-version-and-catalog-policies.md)), so the rule ID set stays
small without losing precision.

### Consequences

* Good, because severity and suppression behave identically for every rule, including ones a host adds through
  its own `RuleProvider`.
* Good, because the JS rules plug in without the core knowing they exist, which is what keeps `js-analysis` an
  optional feature.
* Good, because one diagnostic type feeds every reporter and every host.
* Bad, because renaming breaks sherif configurations outright. Accepted under [ADR 0001](0001-mission-and-scope.md); the rule docs list the
  sherif name for each renamed rule.
* Bad, because freezing IDs makes naming mistakes permanent. New IDs are reviewed against this ADR's
  conventions before their first release.
* Bad, because sorting after collection means no diagnostic is visible until every rule finishes. For the
  budgets in [ADR 0001](0001-mission-and-scope.md) this is invisible.

### Confirmation

* The registry rejects a duplicate rule ID at construction.
* `docs/rules.md` is generated from the registry's `RuleMeta` and checked for freshness in CI.
* Diagnostics are sorted by path, span start, rule ID and message, so output is identical across runs and thread counts.
* New IDs are reviewed against the conventions in this ADR before their first release.

## Pros and Cons of the Options

### Frozen kebab-case rule IDs, a RuleProvider registry and a central Emitter

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Keep sherif IDs for continuity

* Bad, because it buys continuity with a tool skif does not claim to be, at the price of a misspelled ID in public API forever.

### Accept old IDs as aliases

* Bad, because aliases are permanent, double the documented surface, and give sherif configurations a half-working compatibility skif does not otherwise offer.

### Rules apply their own severity and ignores

* Bad, because it is the inherited design: every rule reimplements it slightly differently.

### Numeric IDs (`SK001`)

* Bad, because it is unreadable in a config file and in a terminal; kebab-case IDs are self-describing and match the conventions of the tools skif replaces.

## More Information

* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0012](0012-reporting.md)
* See the [ADR index](index.md).
