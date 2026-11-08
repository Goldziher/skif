---
status: accepted
date: 2026-11-08
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0058"]
---

# `skif update --interactive`: A Terminal Picker on the Update Report

## Context and Problem Statement

[ADR 0058](0058-registry-updates-an-opt-in-skif-update-command.md) added `skif update` and deferred `--interactive`, syncpack's picker for choosing which updates to write, as "a CLI feature on top of the report". Issue [#34](https://github.com/Goldziher/skif/issues/34) lists it as the last missing part of `syncpack update`, and `skif migrate` told users of `syncpack update --interactive` that skif had no equivalent.

Where does the choice happen, and how does it behave without a terminal or with `--check`?

## Decision Drivers

* The library does no I/O and asks no questions ([ADR 0003](0003-library-first-embedding.md))
* A script or CI run must never wait for input
* Cancelling must write nothing
* No new dependency

## Considered Options

* Choose in the CLI on a filtered copy of the update report
* Give the library planner a selection callback

## Decision Outcome

Chosen option: "Choose in the CLI on a filtered copy of the update report", because the planner and `skif::update::apply_updates` stay unchanged and the prompt stays in `skif-cli`.

* `skif update --interactive` plans as usual, then shows one line per dependency, current specifier and target specifier, with the places it changes, sorted and with control characters escaped. Nothing is chosen at the start. The chosen lines' updates are kept in a copy of the report, which `apply_updates` writes.
* Esc or Ctrl-C writes nothing and exits `0`. The human output counts the updates not chosen, and the JSON document gets a `declined` count.
* It needs a terminal on both stdin and stdout; otherwise it exits `2` with error kind `usage` before the CI check, any registry request or any write.
* `--interactive` conflicts with `--check`.
* The prompt is `inquire`'s multi-select, which `skif-cli` already uses for `lint --fix`, so `Cargo.lock` does not change.

### Consequences

* Good, because a syncpack user's `update --interactive` has an equivalent, and `skif migrate` points at it.
* Good, because hosts of the library are unaffected: the selection is a filter over a report they already get.
* Bad, because the picker cannot be scripted; `--dependencies` and `--target` remain the scripted way.

### Confirmation

* Unit tests in `crates/skif-cli/src/update.rs` drive `select` with a scripted `Picker` and a fake registry: one line per dependency across packages, only the chosen line written, nothing written for no choice or an unknown line.
* `crates/skif-cli/tests/update.rs` checks the refusal without a terminal (exit `2`, no registry request, files unchanged) and the conflict with `--check`.
* The CLI reference test fails when the flag is missing from `reference/cli.mdx`.

## Pros and Cons of the Options

### Choose in the CLI on a filtered copy of the update report

* Good, because no library type or function changes.
* Bad, because the CLI keeps its own grouping of updates into lines.

### Give the library planner a selection callback

* Good, because every host would get the same grouping.
* Bad, because the planner would call back into interactive code, which ADR 0003 keeps out of the library.
