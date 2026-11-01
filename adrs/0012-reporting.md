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

The CLI selects with `--format`; when `GITHUB_ACTIONS=true` and no format is given, it prints
`human` to stdout **and** the `github` annotations, so a plain `skif` step annotates the diff.
Renderers do no IO of their own and never read the environment; the CLI resolves TTY, colour and CI
detection and passes them as options ([ADR 0003](0003-library-first-embedding.md)).

### JSON contract

```json
{
  "version": 1,
  "skif": "0.1.0",
  "summary": { "errors": 3, "warnings": 1, "fixable": 2 },
  "diagnostics": [
    {
      "rule": "version-mismatch",
      "severity": "error",
      "message": "react is declared with 2 different versions",
      "help": "align on ^18.3.1",
      "location": { "path": "packages/web/package.json",
                    "span": { "start": 120, "end": 137,
                              "startLine": 7, "startCol": 5, "endLine": 7, "endCol": 22 } },
      "package": "web",
      "dependency": "react",
      "detail": { "state": "DiffersToHighestOrLowestSemver" },
      "fixes": [ { "title": "Use ^18.3.1", "safety": "unsafe" } ]
    }
  ]
}
```

* `version` is the schema version. Adding a field is not a breaking change; removing or renaming
  one, or changing a field's type or meaning, bumps `version`.
* Diagnostics are in the deterministic order of [ADR 0007](0007-rule-and-diagnostic-model.md), so two runs over the same tree produce
  byte-identical output.
* Paths are workspace-relative with `/` separators on every OS.
* `detail` objects are per rule and documented with the rule; their keys follow the same stability
  rule.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | no diagnostic at `error` severity (warnings alone do not fail) |
| `1` | at least one `error` diagnostic remains after any fixes were applied |
| `2` | skif could not run: invalid configuration, both config files present, unreadable root, bad arguments |

`--max-warnings <n>` turns more than `n` warnings into exit `1`, for teams that ratchet warnings
down.

The rule reference `docs/rules.md` is generated from the registry's `RuleMeta` and checked for
freshness in CI.

### Consequences

* Good, because A host uses `Report` directly and can still reuse any renderer, writing to its own buffer.
* Good, because GitHub users get inline annotations with no extra configuration.
* Good, because renderers are unit-testable against an in-memory `Vec<u8>`, and the JSON output is snapshot
  tested with paths redacted.
* Bad, because the JSON shape is now a public API with its own version, maintained alongside the crate's semver.
* Bad, because SARIF consumers differ in which optional fields they read; only the fields GitHub code scanning
  uses are guaranteed.
* Bad, because exit code `2` for configuration errors means a hook fails closed when the config is broken,
  which is intended but surprising when a teammate commits an invalid `skif.toml`.

### Confirmation

* Renderers are unit-tested against an in-memory `Vec<u8>`.
* The JSON output is snapshot tested with paths redacted.
* `docs/rules.md` is checked for freshness in CI.
* Diagnostics follow the deterministic order of [ADR 0007](0007-rule-and-diagnostic-model.md), so two runs over the same tree produce byte-identical output.

## Pros and Cons of the Options

### Four renderers over a Report, a versioned JSON contract and three exit codes

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Exit `1` for warnings too

* Bad, because warnings would be errors in all but name; `--max-warnings` covers the teams that want it.

### NDJSON, one diagnostic per line

* Bad, because the summary and version need a container; streaming can be added as another format if needed. (rejected for the default JSON)

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0014](0014-git-hook-integration.md)
* See the [ADR index](index.md).
