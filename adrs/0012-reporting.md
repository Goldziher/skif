---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Reporting: Four Renderers, Stable JSON, Three Exit Codes

## Context and Problem Statement

skif runs in three places: a developer's terminal, a git hook ([ADR 0014](0014-git-hook-integration.md)), and CI. Each wants
something different. A terminal wants grouped, coloured output with the offending line; CI wants
annotations on the pull request diff and an upload format for code-scanning dashboards; scripts
want a format they can parse without regexes.

The inherited printer writes coloured text straight to stdout and stderr from inside the lint path
(`printer.rs`), with a `json` flag added later, so output cannot be redirected, captured by a host,
or tested without spawning the binary.

## Decision Drivers

* Terminals, git hooks and CI each want a different output
* Scripts need a format they can parse without regexes
* The inherited printer wrote straight to stdout and stderr, so output could not be redirected, captured or tested

## Considered Options

* Four renderers over a Report, a versioned JSON contract and three exit codes
* Exit `1` for warnings too
* NDJSON, one diagnostic per line

## Decision Outcome

Chosen option: "Four renderers over a Report, a versioned JSON contract and three exit codes", because renderers that do no IO can be reused by hosts and unit-tested, GitHub users get annotations with no configuration, and a versioned JSON shape gives scripts a stable contract.

`skif-report` provides **four renderers**, each a function from a `Report` to `&mut dyn Write`:

| Format | For | Notes |
| --- | --- | --- |
| `human` | terminals | grouped by file, source excerpt with the span underlined, `help`, fix availability; colour only when the writer is a TTY and `NO_COLOR` is unset (decided by the CLI and passed in) |
| `json` | scripts, hosts | the stable contract below |
| `sarif` | code scanning | SARIF 2.1.0; one `run`, rules from `RuleMeta`, `physicalLocation` regions from `Span` |
| `github` | GitHub Actions | `::error file=…,line=…,col=…,endLine=…,endColumn=…,title=<rule>::<message>` workflow commands |

The CLI selects with `--format` and can send the rendering to a file with `--output`, which is how
a SARIF file reaches an upload step ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)); when `GITHUB_ACTIONS=true` and no format is given, it
prints `human` to stdout **and** the `github` annotations, so a plain `skif` step annotates the diff.
Renderers do no IO of their own and never read the environment; the CLI resolves TTY, colour and CI
detection and passes them as options ([ADR 0003](0003-library-first-embedding.md)).

### JSON contract

```json
{
  "version": 1,
  "skif": "0.1.0",
  "summary": { "errors": 3, "warnings": 1, "infos": 0, "fixable": 2, "packages": 12 },
  "diagnostics": [
    {
      "rule": "version-mismatch",
      "severity": "error",
      "message": "react is declared with 2 different versions",
      "help": "align on ^18.3.1",
      "location": { "path": "packages/web/package.json",
                    "span": { "start": 120, "end": 137,
                              "start-line": 7, "start-col": 5, "end-line": 7, "end-col": 22 } },
      "package": "web",
      "dependency": "react",
      "detail": "DiffersToHighestOrLowestSemver",
      "fixes": [ { "title": "Use ^18.3.1", "safety": "unsafe" } ]
    }
  ]
}
```

* `version` is the schema version. Adding a field is not a breaking change; removing or renaming
  one, or changing a field's type or meaning, bumps `version`. Keys are kebab-case, like the rest of
  skif's serialized data.
* `severity` is `error`, `warning` or `info`, and `summary` counts each. `span` offsets are `u32`
  UTF-8 byte offsets, lines and columns are 1-based, and columns count Unicode scalar values
  ([ADR 0007](0007-rule-and-diagnostic-model.md)); SARIF regions are converted to UTF-16 columns by the renderer.
* Diagnostics are in the deterministic order of [ADR 0007](0007-rule-and-diagnostic-model.md), so two runs over the same tree produce
  byte-identical output.
* Paths are workspace-relative with `/` separators on every OS.
* `detail` is a string; for policy diagnostics it is the syncpack state name ([ADR 0009](0009-version-and-catalog-policies.md)), and the
  set of values per rule is documented with the rule and follows the same stability rule.

**JSON mode carries every message as JSON.** When the format is `json`, stdout holds exactly one
JSON document per run and nothing else, in one of these shapes, each with `version` and `skif`:

| Shape | When | Extra keys |
| --- | --- | --- |
| report | a lint run, with or without `--fix` | `summary`, `diagnostics`, and with `--fix` a `fix` object: `applied` and `skipped` fixes (each with its diagnostic and reason), `changed` paths, and an `install` object (`status` of `ran`, `skipped` or `failed`, plus the `command` and, on failure, its `message`) |
| error | skif could not run (exit code 2) | `error`: `{ "kind": "config" \| "workspace" \| "io" \| "usage" \| "ci-refusal", "message", "path"?, "line"?, "column"? }` |

Human text is never mixed into JSON-mode stdout; a warning that would have been a stderr line
becomes a diagnostic or a field of the document ([ADR 0003](0003-library-first-embedding.md)).

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | no `error` diagnostic remains, and warnings do not exceed the allowed number (`info` never counts) |
| `1` | at least one `error` diagnostic remains after any fixes were applied, or there are more warnings than `--max-warnings` (or any warning with `fail-on-warnings`) allows |
| `2` | skif could not run: invalid configuration, both config files present, unreadable root, bad arguments, `--fix` refused in CI, the post-fix install failed |

`--max-warnings <n>` turns more than `n` warnings into exit `1`, for teams that ratchet warnings
down; `[lint] fail-on-warnings = true` is `--max-warnings 0`. Exit `2` is reserved for the tool
failing, so a script can tell "the repository has problems" from "skif did not check it"
([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)).

The rule reference `docs/rules.md` is generated from the registry's `RuleMeta` and checked for
freshness in CI.

### Consequences

* Good, because a host uses `Report` directly and can still reuse any renderer, writing to its own buffer.
* Good, because GitHub users get inline annotations with no extra configuration.
* Good, because renderers are unit-testable against an in-memory `Vec<u8>`, and the JSON output is snapshot
  tested with paths redacted.
* Bad, because the JSON shape is now a public API with its own version, maintained alongside the crate's semver.
* Bad, because SARIF consumers differ in which optional fields they read; only the fields GitHub code scanning
  uses are guaranteed.
* Bad, because exit code `2` for configuration errors means a hook fails closed when the config is broken,
  which is intended but surprising when a teammate commits an invalid `skif.toml`.

### Confirmation

* Each renderer in `skif-report` has a unit test that renders a fixed `Report` into a `Vec<u8>`.
* An insta snapshot test of the JSON renderer, with paths redacted, covers the report shape including `infos`, a string `detail` and kebab-case span keys; a second snapshot covers the error shape for each `kind`.
* A SARIF test validates the rendered document against the SARIF 2.1.0 JSON Schema and checks UTF-16 columns on a line with an astral character.
* CLI tests (`insta-cmd`) assert the exit code for each row of the exit-code table, including `--max-warnings` and `info`-only reports, and that JSON-mode stdout parses as exactly one JSON document in every case, including an invalid `skif.toml`.
* A determinism test renders two runs over the same tree and asserts byte-identical output.
* The `docs/rules.md` freshness test regenerates the reference from the registry and fails on any difference.

## Pros and Cons of the Options

### Four renderers over a Report, a versioned JSON contract and three exit codes

* Good, because renderers do no IO, so hosts reuse them and tests run in memory; GitHub users get annotations with no configuration; scripts get a versioned, parseable contract, and exit `2` separates a broken run from a dirty repository.
* Bad, because the JSON shape is a public API with its own version, SARIF consumers read different optional fields, and a broken `skif.toml` makes a hook fail closed.

### Exit `1` for warnings too

* Good, because nothing would pass with warnings left behind.
* Bad, because warnings would be errors in all but name; `--max-warnings` covers the teams that want it.

### NDJSON, one diagnostic per line

* Good, because output could stream and a consumer could stop reading early.
* Bad, because the summary and version need a container; streaming can be added as another format if needed. (rejected for the default JSON)

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0014](0014-git-hook-integration.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* See the [ADR index](index.md).
