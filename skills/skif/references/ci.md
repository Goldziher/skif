# skif in CI and git hooks

The full guide is <https://goldziher.github.io/skif/guides/integrations/>.

## GitHub Action

```yaml
- uses: actions/checkout@v7
- uses: Goldziher/skif@v0.3
  with:
    fail-on-warnings: true
```

Inputs: `version` (default: the version the action release was published with), `args` (extra skif arguments; one
line is split on spaces, several lines are one argument each), `working-directory`, `fail-on-warnings`,
`from-sherif` (lint with the translated sherif settings while the project has no `skif.toml`), `install-only`,
`token`, `repository`, `verify-attestation`. Outputs: `version`, `path`.

A floating tag (`@v0.M` for a 0.x line, `@vN` from 1.0) takes every patch release of its line; a full tag or a commit
SHA pins one version. On GitHub Actions, with no `--format`, skif prints the human report plus workflow annotations.

Code scanning:

```yaml
- uses: Goldziher/skif@v0.3
  with:
    args: --format sarif --output skif.sarif
- uses: github/codeql-action/upload-sarif@v4
  if: always()
  with:
    sarif_file: skif.sarif
```

## Other CI systems

Run the npm package or a release binary: `npx skif-cli --format codeclimate --output gl-code-quality.json` feeds
GitLab Code Quality. One run can write several reports:
`skif --format human --format json=skif.json --format sarif=skif.sarif`. `--format markdown` (or `disclosure`)
renders a pull request comment, and `--format codeowners` groups findings by CODEOWNERS owner.

`--fix` and `--fix-unsafe` exit `2` when `CI` is set; lint only in CI, and use `--fix --dry-run` to show the diff.

## Ratchet with a baseline

```yaml
- run: git fetch origin main
- uses: Goldziher/skif@v0.3
- uses: Goldziher/skif@v0.3
  with:
    args: baseline check --base origin/main
```

The first step lints with the baseline applied; the second fails when the baseline gained entries. See
[migrate.md](migrate.md#adopting-skif).

## pre-commit and prek

```yaml
repos:
  - repo: https://github.com/Goldziher/skif
    rev: v0.3.0 # use the latest release tag
    hooks:
      - id: skif       # skif --no-js
      - id: skif-fix   # skif --fix --no-install --no-prompt --no-js (opt-in, rewrites manifests)
```

The hooks need neither Rust nor Node: they use `SKIF_BINARY`, else `skif` on `PATH`, else the release pinned by `rev`,
downloaded once and verified. They run when `package.json`, `pnpm-workspace.yaml`, `skif.toml` or `.skif.toml`
change, and skip the JavaScript analysis (`--no-js`), which belongs in CI.

## Poly and Lefthook

Poly, in `poly.toml`:

```toml
[[hooks.sources]]
id = "skif"
git = "https://github.com/Goldziher/skif.git"
revision = "v0.3.0" # use the latest release tag
hooks = ["skif"]
```

Lefthook, in `lefthook.yml`:

```yaml
pre-commit:
  commands:
    skif:
      glob: "{package.json,**/package.json,pnpm-workspace.yaml,skif.toml,.skif.toml}"
      run: skif --no-js
```
