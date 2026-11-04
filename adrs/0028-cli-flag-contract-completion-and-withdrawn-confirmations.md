---
status: accepted
date: 2026-11-04
decision-makers: ["Na'aman Hirschfeld"]
---

# CLI Flag Contract Completion and Withdrawn Confirmations

## Context and Problem Statement

[ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) is the CLI contract, but its flag table omits two flags the CLI has, `--no-prompt` and `--color`, and says colour is decided by the terminal alone. [ADR 0024](0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md) added `--config` and `--no-config`. Separately, several confirmation lines in locked records name artefacts that do not exist, and the index of effective statements covers only some of them. A reader of those records sees confirmations that are silently dead.

## Decision Drivers

* The flag contract in one record set must list every flag the CLI accepts
* A confirmation line must describe something that runs, or be marked withdrawn
* Locked records stay unedited; corrections live in a new record and the index

## Considered Options

* Complete the flag contract and withdraw the dead confirmations in one record
* Edit the locked records
* Leave the records as written

## Decision Outcome

Chosen option: "Complete the flag contract and withdraw the dead confirmations in one record", because it keeps the records immutable and tells a reader which statement governs.

**Flags added to the ADR 0016 table.**

| Flag | Meaning |
| --- | --- |
| `--no-prompt` | never ask which alternative fix to apply; a diagnostic with several alternatives stays unfixed unless `--select` decides. Both hook definitions pass it. |
| `--color <auto\|always\|never>` | when to colour human output. `auto` is the behaviour in ADR 0016 (a TTY, `NO_COLOR` unset, no `--output`); `always` and `never` override it. |
| `--config <FILE>`, `--no-config` | the configuration flags of ADR 0024; they conflict. |

**Withdrawn confirmations.** These lines of earlier records do not apply:

* [ADR 0011](0011-plugin-model.md): the fixture directories `fixtures/js/plugins/<id>/` do not exist; plugin tests in `crates/skif-js-plugins/tests` use in-memory file systems. The rustdoc-JSON check and `mise run parity` were withdrawn by [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md). The plugin count is 25 ([ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md)).
* [ADR 0010](0010-js-analysis-on-oxc.md) and [ADR 0021](0021-oxc-lockstep-with-poly.md): the `skif-js-syntax` rustdoc-JSON check does not exist (withdrawn by ADR 0019). The oxc types stay out of the public API by review.
* [ADR 0002](0002-crate-layout-and-publishing.md): there is no `skif-testing` crate (ADR 0019).
* [ADR 0003](0003-library-first-embedding.md): the `clippy.toml` `disallowed-methods` list and the `print_stdout` and `print_stderr` denials do not exist (ADR 0019). The claim that the library never prints or reads the environment holds except for the opt-in `SKIF_TIMINGS` timings.

### Consequences

* Good, because the flag contract and the dead confirmations are readable from the index without opening every record.
* Bad, because ADR 0016 must be read together with this record and ADR 0024.

### Confirmation

* `crates/skif-cli/src/args.rs` defines exactly the flags in the ADR 0016 table plus the four above, and `website/src/content/docs/reference/cli.mdx` lists them.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Complete the flag contract and withdraw the dead confirmations in one record

* Good, because records stay locked and the index says what governs.
* Bad, because the truth is split across records.

### Edit the locked records

* Good, because each record would be self-contained.
* Bad, because it breaks the immutability that `adrlint.lock` enforces.

### Leave the records as written

* Good, because it needs no work.
* Bad, because dead confirmations and missing flags mislead readers.

## More Information

* Amends: [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), [ADR 0011](0011-plugin-model.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0021](0021-oxc-lockstep-with-poly.md), [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0003](0003-library-first-embedding.md)
* Related decisions: [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md), [ADR 0024](0024-layered-configuration-cascading-files-extends-overrides-and-package-files.md)
* See the [ADR index](index.md).
