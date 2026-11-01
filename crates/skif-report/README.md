# skif-report

Renderers that turn a `skif_core::Report` into human-readable or JSON output.

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
  "skif": "0.1.0",
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
reported as `{"version": 1, "error": {"message": "..."}}`.
