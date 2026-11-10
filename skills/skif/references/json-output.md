# Reading skif's JSON output

With `--format json`, stdout holds exactly one JSON document in every case, failures included. Nothing else is
printed there; notes and timings go to stderr.

## The lint report

```json
{
  "version": 1,
  "skif": "<skif version>",
  "summary": { "errors": 1, "warnings": 1, "infos": 0, "fixable": 2, "packages": 3 },
  "diagnostics": [
    {
      "rule": "types-in-dependencies",
      "severity": "error",
      "message": "private package has `@types/node` in `dependencies`",
      "help": "move `@types/*` packages to `devDependencies`; ...",
      "location": {
        "path": "packages/a/package.json",
        "span": { "start": 87, "end": 110, "start-line": 6, "start-col": 5, "end-line": 6, "end-col": 28 }
      },
      "related": [],
      "package": "@acme/a",
      "dependency": "@types/node",
      "specifier": "22.0.0",
      "fixes": [{ "title": "Move the `@types/*` packages to `devDependencies`", "safety": "unsafe" }]
    }
  ]
}
```

- `severity` is `error`, `warning` or `info`. `summary.fixable` counts the diagnostics that have a fix.
- `help`, `package`, `dependency`, `specifier` and `detail` are present only when known. `span` is missing for a
  finding about a whole file. `related` holds other locations, each with a `message` and a `location`.
- `fixes` lists the fixes `--fix` could apply: `safety` is `safe` (applied by `--fix`) or `unsafe` (needs
  `--fix-unsafe`). Several entries are alternatives; `--select` or `[policy] prefer` decides between version
  alignments, and an interactive run asks. An empty list means the finding has no automatic fix.
- A multi-root run (`-r`) adds `roots`, one object per root with its `path`, `packages` and `summary`, and `errors`
  for roots that could not be loaded (the run then exits `2`, the other roots are still reported).
- `incomplete-packages` (omitted when empty) lists the packages whose entry files skif could not fully determine,
  for example because a tool config is computed at run time. In those packages unused dependencies are not reported
  and unused files only outside `scope`. Each entry has `name`, `dir`, `reasons` (each a `cause` with `plugin` and
  `file` when known) and `scope`: `"package"` when nothing in the package was judged unused, or
  `{"within": [globs]}` for the only files left unjudged. A clean report with incomplete packages is not proof that
  nothing is unused there.

With `--fix`, the document adds a `fix` object: `applied` (`rule`, `message`, `title`), `skipped` (`rule`, `message`, a
`reason` such as `safety` or `removes-files`, and a `detail`), `changed` (the files written) and `install` (`status`
and `reason`). With `--fix --dry-run` it holds `"dry-run": true` and `changes` (`path`, `kind` `modify`, `create` or
`delete`, and a unified `diff`) instead of writing. The `diagnostics` of a fix run come from the lint after the fixes.

## The error document

When skif cannot run (exit `2`), the document is:

```json
{ "version": 1, "skif": "<skif version>", "error": { "kind": "config", "message": "...", "path": "skif.toml", "line": 3, "column": 1 } }
```

`kind` is `config`, `workspace`, `io`, `usage`, `ci-refusal` or `internal` (a bug in skif). `path`, `line` and
`column` are present when known. Fix the named file or flag and run again.

## Other JSON outputs

- `skif rules --format json`: `{"version", "skif", "rules": [{"id", "category", "default", "fixable", "summary"}]}`;
  `default` is `null` for a rule that is off by default.
- `skif explain-usage <FILE[#EXPORT]> --format json`: a `report` with `file`, `in-graph`, `entry` (why the file is an
  entry, or `unknown`), `imported-by` (`file`, `line`, `kind`, `names`, `reachable`), `chain-from-entry` and, with
  `#EXPORT`, `export` with a `verdict` of `public`, `used`, `unused` or `not-judged`. A file outside the module graph
  exits `2` with an `error` document whose `report.absence` says why (`unused-file`, `ignored`, `not-discovered`,
  `entries-incomplete`, ...).
- `skif --trace-dependency <NAME> --format json`: a `traces` array, one entry per root.
- `skif config`, `skif migrate`, `skif baseline` and `skif update` with `--format json` print one document each,
  with the same `version` and `skif` members; their fields are listed in the
  [CLI reference](https://goldziher.github.io/skif/reference/cli/).
- `skif json` prints NDJSON: one object per dependency instance, one per line.

The MCP server (`skif mcp`) returns these same documents from its `lint`, `explain_usage` and `list_rules` tools.
