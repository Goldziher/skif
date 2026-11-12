---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Rule and Diagnostic Model, Rule Ids and Severities

## Context and Problem Statement

Rule ids are what users type: in `[rules]`, in `[[ignore]]`, in CI scripts that read JSON output, in SARIF and in
baselines. Once published they cannot change without breaking every one of those. Several of sherif's ids are misspelled
or unclear (`non-existant-packages` describes a pattern, `multiple-dependency-versions` is one case of a version
policy), and sherif's rules each apply their own severity and ignore logic, so there is no uniform finding a host or a
renderer can consume.

skif's rules come from several crates, and the JS rules live in a crate most hosts do not compile
([ADR 0003](0003-library-api-and-embedding.md)), so the registry cannot be a hard-coded list. A first run on a real
repository should also fail on dependency problems, not on dead-code heuristics whose precision varies between projects.
How are rules identified, registered and run, what does a finding look like, and how severe is each by default?

## Decision Drivers

* Rule ids are public API and must be self-describing and permanent
* Severity and suppression behave identically for every rule, including a host's own
* Rules plug in from several crates without the core knowing them
* One diagnostic type feeds every renderer and host, with spans a host can map without re-reading the file
* Deterministic output across runs and thread counts
* Default severities that keep a first run actionable

## Considered Options

* Frozen kebab-case ids, a `RuleProvider` registry and a central `Emitter`, with tiered default severities
* Keep sherif's ids for continuity
* Accept sherif's ids as aliases
* Rules apply their own severity and ignores
* Numeric ids (`SK001`)

## Decision Outcome

Chosen option: "Frozen kebab-case ids, a `RuleProvider` registry and a central `Emitter`, with tiered default
severities", because ids are then readable and permanent, one emitter makes severity and suppression the same for every
rule, and the defaults put dependency correctness ahead of heuristics.

### Rule ids

* Every rule has a kebab-case id that never changes once released. A change of meaning gets a new id.
* An id names the problem in the singular when each finding is one instance (`local-path-dependency`) and in the plural
  when it mirrors an upstream issue type users know (`unused-dependencies`). New ids are reviewed against these
  conventions before their first release.
* sherif's ids that skif renamed are retired and not accepted: `unordered-dependencies` is `unsorted-dependencies`,
  `non-existant-packages` is `unmatched-workspace-patterns`, `packages-without-package-json` is
  `missing-package-manifest`, `local-path-dependencies` is `local-path-dependency`, `multiple-dependency-versions` is
  `version-mismatch`, and `unsync-similar-dependencies` is `unsynced-dependency-family`. The rule documentation names
  the sherif id of each renamed rule.
* `rule-ids.txt` lists every released id and is the record of the set; `crates/skif-rules/tests/retired-rule-ids.txt`
  lists each retired id with its replacement. A released id is never removed from `rule-ids.txt`.

### Rules, providers and the registry

* A `Rule` reports its `RuleMeta`, parses its options and checks a `LintContext`, emitting findings. A `RuleProvider`
  reports the metadata of its rules, validates their options and runs the enabled ones; `RulesProvider` adapts a list of
  rules, runs them in parallel and merges their findings in rule order. A host with rules of its own implements
  `RuleProvider`. `ProviderError` is a provider that fails to run, not a finding.
* `skif-rules` provides the manifest, workspace, policy, catalog and toolchain rules (`skif_rules::providers()`);
  `skif-js` provides the JS rules. The facade assembles the registry from the providers its features enable.
* The registry rejects a duplicate id and is the one source for `skif rules`, the schema's rule list and the generated
  `docs/rules.md`. It validates `[rules]` and `[[ignore]]` ids with a closest-match suggestion and deserializes each
  configured rule's options ([ADR 0004](0004-configuration-model-and-layering.md)), so a misconfiguration fails
  `Linter::new`.
* `RuleMeta` holds the id, the `Category` (`workspace`, `manifest`, `policy`, `catalog`, `js`), the default severity
  (`None` is off by default), the `Fixability` (`none`, `safe`, `unsafe`, `sometimes`), a one-line summary, and
  `version_scoped`, which says whether a finding is about the specifier its dependency is written with; it is not
  serialized and tells baselines how to key the finding ([ADR 0023](0023-baselines.md)).

### The emitter owns policy, rules own detection

A rule emits every problem it finds and does nothing else. The `Emitter`, forked per rule and merged afterwards,
applies centrally:

* the effective severity: the configured level, else the severity the rule proposed for the finding (so a rule can
  derive its default from other settings such as `[catalog] mode`), else the rule's default; then the levels and caps of
  the overrides that cover the finding and `[lint] max-level`. A rule whose level is `off` does not run.
* `[[ignore]]` entries, matched by `rules`, `packages` (by `PackageId`, not display name), `dependencies` (a name, or
  `name@specifier`) and `files`, an override's entries only within its scope;
* `[workspace] ignore-packages`.

The toolchain rules also honour comment suppressions in the files they read
([ADR 0011](0011-lockfile-and-toolchain-rules.md)).

### The diagnostic

* `Diagnostic` holds `rule`, `severity` (`Error`, `Warning`, `Info`), a one-line `message`, `help`, `location` (path and
  optional span), `related` locations, the display name of the `package` and its in-process `PackageId`, the
  `dependency` and the `specifier` it is written with, `detail` (a machine-readable discriminator, such as the syncpack
  state behind a policy finding, that keeps the id set small), and `fixes`, the first preferred
  ([ADR 0009](0009-fixes-edits-and-package-json-formatting.md)).
* `Info` never fails a run. `Report` summarises errors, warnings, infos, fixable findings and packages.
* Findings are sorted by path, span start, rule id and message, so output is identical across runs and thread counts. A
  cancel flag is checked between rules.
* `Span` holds half-open `u32` UTF-8 byte offsets (a larger source is rejected, not truncated) and 1-based lines and
  columns that count Unicode scalar values. A `LineIndex` built once per file converts offsets and also gives UTF-16
  columns for the formats that count that way. Paths are root-relative with `/` separators.
* Public enums that will grow (`Category`, `Fixability`, the error types) are `#[non_exhaustive]`.

### Default severities

* The JS rules are tiered by how much their precision depends on entry-point and binary detection, which varies most
  between projects:
  * error: `unused-dependencies`, `unlisted-dependencies`, `unresolved-imports` and `stale-mocks`, which are about
    dependency correctness;
  * warning: `unused-files`, `unused-exports`, `unused-types`, `unlisted-binaries`, `unused-dev-dependencies`,
    `unused-optional-peer-dependencies`, `unused-enum-members`, `unused-namespace-members`, `unused-namespace-exports`,
    `unused-namespace-types`, `unused-subpath-imports`, `duplicate-exports`, `unparseable-files` and
    `stale-js-config`;
  * off: `circular-imports` and `unused-subpath-exports`.
* Every other rule takes its default from the record that decides it. The generated `docs/rules.md` states the default
  of every rule.
* `invalid-workspace-file` (`workspace`, error, no fix) reports every `manifest_errors` entry of discovery
  ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)) as a finding, so a broken member manifest exits 1
  like any error finding, not 2 like a failed run.
* `[rules]`, `--rule`, `--fail-on-warnings` and `--max-warnings` change the outcome
  ([ADR 0024](0024-cli-contract.md)).

### Consequences

* Good, because severity and suppression behave identically for every rule, including a host's.
* Good, because the JS rules plug in without the core knowing them, which keeps `js-analysis` optional.
* Good, because one diagnostic type feeds every renderer and host, and options are validated once.
* Good, because a first run on a real repository fails on dependency problems, not on dead-code heuristics.
* Bad, because renaming breaks sherif configurations outright; `skif migrate` translates them
  ([ADR 0025](0025-skif-migrate.md)).
* Bad, because freezing ids makes naming mistakes permanent.
* Bad, because a custom provider must validate options consistently with its rules.
* Bad, because sorting after collection shows no finding until every rule has finished.
* Bad, because the warning tier hides real unused exports from a run without `--fail-on-warnings`.

### Confirmation

* `crates/skif-rules/tests/rule_ids.rs`: `every_released_rule_id_is_still_registered`,
  `every_registered_rule_id_is_listed_as_released`, `rule_ids_are_listed_once` and
  `retired_ids_are_gone_and_point_at_a_registered_replacement`.
* `scripts/check-rule-ids.sh` fails when an id of `rule-ids.txt` is no longer documented in `docs/rules.md`, when the
  document has an id the file does not, or when the registered `RuleMeta` statics differ from the file.
* `crates/skif/tests/docs.rs`: `rules_reference_is_up_to_date` regenerates `docs/rules.md` from the registry and fails
  on any difference.
* `crates/skif-lint/tests/registry.rs`: `duplicate_rule_ids_are_rejected`, `unknown_rule_in_config_is_a_config_error`,
  `registry_validates_options_through_the_owning_rule` and `disabled_rules_do_not_run_and_severity_is_applied`.
* `crates/skif-lint/tests/severity.rs`: `a_proposed_severity_replaces_the_rule_default` and
  `a_configured_level_beats_the_proposed_severity`; `crates/skif-lint/tests/ignore.rs`: `ignore_matching_table`.
* `crates/skif-lint/tests/run.rs`: `report_is_the_sorted_merge_and_identical_across_runs_and_pools`.
* `crates/skif-core/src/span.rs` unit tests: `crlf_keeps_carriage_return_on_previous_line` and
  `astral_chars_are_one_column_but_two_utf16_units`.

## Pros and Cons of the Options

### Frozen kebab-case ids, a `RuleProvider` registry and a central `Emitter`, with tiered default severities

* Good, because ids are self-describing and permanent, and suppression is uniform.
* Bad, because a naming mistake is permanent and the central emitter delays output until every rule finishes.

### Keep sherif's ids for continuity

* Good, because sherif configurations would keep working.
* Bad, because it buys continuity with a tool skif does not claim to be, at the price of a misspelled id forever.

### Accept sherif's ids as aliases

* Good, because migration would be gradual.
* Bad, because aliases are permanent, double the documented surface, and give sherif configurations a half-working
  compatibility skif does not otherwise offer.

### Rules apply their own severity and ignores

* Good, because a rule can special-case its own suppression.
* Bad, because every rule reimplements it slightly differently.

### Numeric ids (`SK001`)

* Good, because they are short and survive a rename of the problem.
* Bad, because they are unreadable in a configuration file and a terminal.
