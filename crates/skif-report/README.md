# skif-report

Renderers that turn a `skif_core::Report` into human-readable, JSON, SARIF, GitHub annotation, Code Climate,
compact, Markdown or CODEOWNERS-grouped output.

Renderers write to a `&mut dyn std::io::Write`, never touch the environment or the terminal, and take
a `RenderOptions` that the caller (normally the CLI) fills in.

```rust
use skif_report::{Format, RenderOptions, render};

# fn demo(report: &skif_core::Report) -> std::io::Result<()> {
let mut out = Vec::new();
render(report, Format::Json, &RenderOptions::default(), &mut out)?;
# Ok(())
# }
```

## JSON schema (version 1)

```json
{
  "version": 1,
  "skif": "<skif version>",
  "summary": { "errors": 1, "warnings": 0, "infos": 0, "fixable": 1, "packages": 3 },
  "diagnostics": [
    {
      "rule": "version-mismatch",
      "severity": "error",
      "message": "...",
      "help": "...",
      "location": { "path": "packages/web/package.json",
                    "span": { "start": 120, "end": 137, "start-line": 7, "start-col": 5, "end-line": 7, "end-col": 22 } },
      "related": [ { "message": "...", "location": { "path": "..." } } ],
      "package": "web",
      "dependency": "react",
      "detail": "...",
      "fixes": [ { "title": "...", "safety": "safe" } ]
    }
  ]
}
```

`skif`, `help`, `span`, `package`, `dependency` and `detail` are omitted when absent; `related` and
`fixes` are always arrays. Paths are relative with `/` separators. Adding a field is not a breaking
change; removing or renaming one, or changing its type or meaning, bumps `version`. Failures are
reported as `{"version": 1, "error": {"kind": "...", "message": "...", "path": "...", "line": 1, "column": 1}}`;
`kind` and `message` are always present and `path`, `line` and `column` only when the failure has a location.

A run over several roots (`skif -r`) adds two top-level members through `render_json_with`, which takes them as
extra fields: `roots`, an array with one `{ "path", "packages", "summary" }` object per root (`packages` lists the
package directories, `summary` has the counts of that root), and `errors`, present only when a root could not be
loaded, with one `{ "path", "kind", "message" }` object per failed root. The other roots are still linted.

## SARIF and GitHub annotations

`Format::Sarif` writes a SARIF 2.1.0 log (one run, one result per diagnostic, with the rule metadata and
the fix titles) for code-scanning upload. `Format::Github` writes one workflow command per diagnostic
(`::error file=...,line=...,col=...::message`), which GitHub shows as annotations on the pull request.

## Code Climate, compact, Markdown and CODEOWNERS

`Format::Codeclimate` writes the Code Climate JSON array GitLab Code Quality reads (`description`, `check_name`,
`fingerprint`, `severity`, `location.path`, `location.lines.begin`). `Format::Compact` writes one
`path:line:col rule message` line per diagnostic. `Format::Markdown` writes a table per rule for a pull request
comment, and `Format::Disclosure` folds each table in a `<details>` section. `Format::Codeowners` groups compact
lines by owner: the caller reads the CODEOWNERS file and passes `CodeOwners::parse(text)` through
`RenderOptions::with_codeowners`; matching follows the gitignore-like rules of GitHub and GitLab, last match wins,
and GitLab section default owners apply to ownerless entries in their section.
