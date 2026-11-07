---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0012", "0016"]
---

# Several Reports in One Run: A Repeatable `--format FORMAT=FILE`

## Context and Problem Statement

[ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) gives the CLI one `--format` and one `--output`, and [ADR 0012](0012-reporting.md) describes four renderers. Since then the renderer set has grown to nine, and a CI job often wants more than one of them from the same lint: a human report in the job log, a JSON report as an artifact and a Code Climate file for GitLab Code Quality, or SARIF for code scanning next to a Markdown comment. Today that takes one run per format, which repeats the whole analysis and can disagree when the tree changes between runs.

How does one run write several reports, and what stays true of stdout, errors and the files when it does?

## Decision Drivers

* One lint must be able to feed several consumers, each with its own file
* Stdout must stay one rendering, so a consumer that parses it is never handed two documents
* A failed run must not leave a fresh report next to a stale one, or half of a set of reports
* The single-format forms, `--format F` and `--format F --output FILE`, must keep their meaning
* Writing files stays in `skif-cli`; renderers do no I/O ([ADR 0003](0003-library-first-embedding.md))

## Considered Options

* A repeatable `--format FORMAT[=FILE]`, at most one format on stdout
* A repeatable `--output`, paired with each `--format` by position
* A separate `--report FORMAT:FILE` flag next to the existing two

## Decision Outcome

Chosen option: "A repeatable `--format FORMAT[=FILE]`, at most one format on stdout", because each value names its format and its destination together, nothing depends on the order of two lists, and the existing forms are its one-value cases.

**Values.** `--format` may be given several times. A value is `FORMAT`, rendered to stdout, or `FORMAT=FILE`, rendered to `FILE`; the file is everything after the first `=`, and an empty one is a usage error. With no value that goes to stdout, stdout stays empty. With no `--format` at all, the default of ADR 0012 holds: `human`, plus the `github` annotations when `GITHUB_ACTIONS=true`.

**Usage errors.** These exit `2` with kind `usage` before anything is linted: two values without a file, two values naming the same file, and `--output` together with more than one `--format` or with a `FORMAT=FILE` value. `--output FILE` stays valid with exactly one `--format FORMAT` and means `FORMAT=FILE`.

**Files only after every rendering.** The CLI renders every requested format into memory first. If any rendering fails (`codeowners` without a CODEOWNERS file, for example), the run exits `2` and no file is written or replaced. Then each file is written atomically, as `--output` is, and stdout is written last, so a failed file write reaches a JSON consumer of stdout as its only document. A write that fails after earlier files were written leaves those files in place; they hold the report of this run.

**JSON mode and colour.** The format that decides how a failure is reported is the one on stdout, or, when no format goes to stdout, the first `--format` given. JSON mode, in which every failure is a JSON `error` document on stdout ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)), is on when that format is `json`. So `--format json=skif.json` alone behaves like `--format json --output skif.json`, and `--format human --format json=skif.json` reports a failure as text on stderr. The same format decides whether fixes may prompt and where the fix summary goes. Colour is decided by `--color` and the terminal only for that format's rendering when it goes to stdout or `--output`; a `FORMAT=FILE` rendering is never coloured.

The exit code does not depend on the formats.

### Consequences

* Good, because one analysis produces every report a pipeline needs, and the reports always describe the same tree.
* Good, because stdout keeps holding at most one rendering, so the JSON contract of ADR 0016 is unchanged.
* Good, because a failed rendering leaves every report file as it was.
* Bad, because a usage error can no longer be caught by clap alone: the stdout and `--output` rules are checked after parsing.
* Bad, because a file name containing `=` is read as part of the file, so a format name can never contain `=`.
* Bad, because the files are not replaced as one transaction: a write that fails midway leaves the earlier files updated.

### Confirmation

* `crates/skif-cli/tests/cli.rs` covers two file renderings next to a human stdout, files only with an empty stdout, two stdout formats and a duplicate file as usage errors, `--output` with several formats or with `FORMAT=FILE`, a failing rendering that writes no file, and JSON mode decided by the stdout format.
* `format_specs_parse` in `crates/skif-cli/src/args.rs` covers the `FORMAT=FILE` syntax.
* `website/src/content/docs/reference/cli.mdx` documents the flag, and `guides/integrations.mdx` shows a GitLab job writing JSON and Code Climate in one run.

## Pros and Cons of the Options

### A repeatable `--format FORMAT[=FILE]`, at most one format on stdout

* Good, because a value is self-contained and the single-format forms keep working.
* Bad, because `=` cannot appear in a format name and the `--output` rule is a cross-flag check.

### A repeatable `--output`, paired with each `--format` by position

* Good, because no new syntax is introduced.
* Bad, because pairing two lists by position is easy to get wrong, and a format meant for stdout has no way to say so in the middle of the list.

### A separate `--report FORMAT:FILE` flag next to the existing two

* Good, because `--format` and `--output` stay exactly as they are.
* Bad, because three flags then decide where output goes, and their combinations need rules of their own.

## More Information

* Amends: [ADR 0012](0012-reporting.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0028](0028-cli-flag-contract-completion-and-withdrawn-confirmations.md)
* See the [ADR index](index.md).
