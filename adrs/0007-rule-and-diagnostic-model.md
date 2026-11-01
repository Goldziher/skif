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
    fn meta(&self) -> &'static RuleMeta;      // id, category, default severity, fixability, summary
    fn parse_options(&self, raw: &RawOptions) -> Result<OptionsBox, OptionsError> { /* default: no options */ }
    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>);
}

pub trait RuleProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn rules(&self) -> Vec<&'static RuleMeta>;
    fn validate_options(&self, rule: &str, raw: &RawOptions) -> Result<(), OptionsError>;
    fn run(&self, cx: &LintContext<'_>, enabled: &EnabledRules, out: &mut Emitter<'_>)
        -> Result<(), ProviderError>;
}
```

A provider reports its rule metadata and runs the rules that are enabled. `RulesProvider` adapts a
list of `Box<dyn Rule>` into a provider: it runs the enabled rules in parallel on the current rayon
pool and merges their diagnostics in rule order, so the result does not depend on scheduling. A
host with rules of its own implements `RuleProvider` directly. `ProviderError` is for a provider
that fails to run, as opposed to a rule that finds a problem. `RuleMeta` is plain `'static` data
(`id`, `Category`, `default: Option<Severity>`, `Fixability`, `summary`).

`skif-rules` provides the manifest and policy rules; `skif-js` provides the JS rules. The facade
assembles a `Registry` from the providers its features enable. The registry rejects a duplicate ID
at construction and is the single source for `skif rules`, the JSON Schema's rule list ([ADR 0004](0004-configuration-model.md))
and the generated `docs/rules.md`. It is also where configuration meets rules: it **validates
`[rules]` and `[[ignore]]`** (unknown rule IDs with a closest-match suggestion) and **deserializes
each configured rule's options** into the type that rule owns ([ADR 0004](0004-configuration-model.md)), so a misconfiguration fails
`Linter::new`, before any rule runs.

`LintContext` is read-only and carries the workspace facts ([ADR 0005](0005-workspace-discovery.md)), the typed configuration, the
resolved `EnabledRules` (the effective severity of every enabled rule), the rule's own typed options
(`cx.options::<T>()`), and the `SpecifierCache` for derived specifiers ([ADR 0006](0006-specifier-and-semver-model.md)).

### The emitter owns policy, rules own detection

A rule calls `out.emit(diagnostic)` for every problem it finds and does nothing else. The
`Emitter`, forked per rule by `skif-lint` and merged afterwards, applies centrally:

* the rule's effective severity, overwriting whatever the rule put in the diagnostic (a rule set to
  `off` is not run at all and its diagnostics would be dropped);
* `[[ignore]]` entries and `workspace.ignore-packages` matching the rule × package × dependency,
  with the package identified by its `PackageId` ([ADR 0005](0005-workspace-discovery.md)) and not by a display name, so two packages
  that share or lack a name are suppressed independently;
* the CLI and host overrides ([ADR 0004](0004-configuration-model.md)).

Rules run in parallel on rayon, and `RunOptions` lets the host supply the thread pool and a cancellation
flag that is checked between rules. Their diagnostics are collected and **sorted deterministically**
by path, span start, rule ID and message, so output is identical across runs and thread counts.
`Diagnostic` has a builder in `skif-core` so rules do not fill ten fields by hand.

### Diagnostic shape

```rust
pub struct Diagnostic {
    pub rule: RuleId,
    pub severity: Severity,             // Error | Warning | Info
    pub message: String,                // one line, no trailing period
    pub help: Option<String>,
    pub location: Location,             // { path, span: Option<Span> }
    pub related: Vec<Related>,          // secondary locations that explain the problem
    pub package: Option<String>,        // display name
    pub dependency: Option<String>,
    pub detail: Option<String>,         // rule-specific machine-readable discriminator
    pub fixes: Vec<Fix>,                // ADR 0008
}

pub struct Span {
    pub start: u32, pub end: u32,       // half-open UTF-8 byte offsets into the file
    pub start_line: u32, pub start_col: u32,
    pub end_line: u32, pub end_col: u32, // 1-based; columns count Unicode scalar values
}
```

**Severity** has three levels. `Error` and `Warning` are what users configure with `error` and
`warn`; `Info` is for observations that never fail a run (`info` in configuration, or a rule's
default). `Report` summarises them as `errors`, `warnings` and `infos`, plus `fixable` and
`packages`.

**Span units are fixed**: offsets are `u32` UTF-8 byte offsets (a source larger than `u32::MAX`
bytes is rejected rather than truncated), lines are 1-based, and columns are 1-based and count
Unicode scalar values, not bytes and not UTF-16 units. A `LineIndex` built once per file converts
offsets, and also provides UTF-16 columns for SARIF ([ADR 0012](0012-reporting.md)), which counts that way. Paths are
workspace-root-relative with `/` separators.

The line and column fields are the four fields of poly's `Span`, and the byte range is the range
poly's `Edit` replaces, so a host maps a skif diagnostic without re-reading the file
([ADR 0003](0003-library-first-embedding.md)). Byte offsets serve edits ([ADR 0008](0008-fix-and-edit-model.md)); line and column serve humans, SARIF and GitHub
annotations ([ADR 0012](0012-reporting.md)). `detail` is a short string that carries machine data such as the exact
syncpack instance state behind a policy diagnostic ([ADR 0009](0009-version-and-catalog-policies.md)), so the rule ID set stays small
without losing precision. Public enums that will gain variants (`Category`, `Fixability`, the error
types) are `#[non_exhaustive]`.

### Consequences

* Good, because severity and suppression behave identically for every rule, including ones a host adds through
  its own `RuleProvider`.
* Good, because the JS rules plug in without the core knowing they exist, which is what keeps `js-analysis` an
  optional feature.
* Good, because one diagnostic type feeds every reporter and every host, and options are validated once,
  in the registry, instead of in every rule.
* Bad, because renaming breaks sherif configurations outright. Accepted under [ADR 0001](0001-mission-and-scope.md); the rule docs list the
  sherif name for each renamed rule.
* Bad, because freezing IDs makes naming mistakes permanent. New IDs are reviewed against this ADR's
  conventions before their first release.
* Bad, because the registry has to know how to validate options for every provider, so a custom provider
  must implement `validate_options` consistently with its rules' `parse_options`.
* Bad, because sorting after collection means no diagnostic is visible until every rule finishes. For the
  budgets in [ADR 0001](0001-mission-and-scope.md) this is invisible.

### Confirmation

* `cargo test -p skif-lint` includes a registry test that a duplicate rule ID, within or across providers, fails `Registry::new`, and tests that an unknown option or rule ID fails with a suggestion.
* An emitter test table in `skif-lint` covers severity override, `off`, `[[ignore]]` by rule, package and dependency, and suppression by `PackageId` for two packages with the same name.
* A determinism test in `skif-lint` runs the same workspace on rayon pools of 1, 2 and 8 threads and asserts byte-identical serialized reports; it injects an out-of-order provider so a merge-order bug is detectable.
* `cargo test -p skif-core` includes `Span` and `LineIndex` tests for multi-byte text (a two-byte, a three-byte and an astral character), CRLF endings and a BOM, asserting byte offsets, scalar-value columns and UTF-16 columns separately.
* The `docs/rules.md` freshness test (run by `mise run test`) regenerates the reference from the registry's `RuleMeta` and fails on any difference.
* A test enumerates every released rule ID from a committed `rule-ids.txt` and fails if one disappears from the registry; the retired-ID list is checked the same way.

## Pros and Cons of the Options

### Frozen kebab-case rule IDs, a RuleProvider registry and a central Emitter

* Good, because IDs are self-describing and permanent, severity and suppression are identical for every rule including a host's own, and the JS rules plug in without the core knowing they exist.
* Bad, because a naming mistake is permanent, renaming breaks sherif configurations outright, and the central emitter means no diagnostic is visible until every rule finishes.

### Keep sherif IDs for continuity

* Good, because existing sherif configurations would keep working.
* Bad, because it buys continuity with a tool skif does not claim to be, at the price of a misspelled ID in public API forever.

### Accept old IDs as aliases

* Good, because migration would be gradual.
* Bad, because aliases are permanent, double the documented surface, and give sherif configurations a half-working compatibility skif does not otherwise offer.

### Rules apply their own severity and ignores

* Good, because a rule can special-case its own suppression.
* Bad, because it is the inherited design: every rule reimplements it slightly differently.

### Numeric IDs (`SK001`)

* Good, because they are short and survive renaming of the problem they describe.
* Bad, because they are unreadable in a config file and in a terminal; kebab-case IDs are self-describing and match the conventions of the tools skif replaces.

## More Information

* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0012](0012-reporting.md)
* See the [ADR index](index.md).
