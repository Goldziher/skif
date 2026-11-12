---
name: skif
description: >-
  Lint the dependencies of JavaScript and TypeScript monorepos with skif: workspace and manifest rules, version and
  catalog policies, and unused or unlisted dependencies, files and exports, with format-preserving fixes. Use when a
  repository has a skif.toml or runs skif in CI or git hooks, when asked to check, fix or clean up package.json
  dependencies, version mismatches, catalogs, unused dependencies, dead files or unused exports in an npm, pnpm, yarn
  or bun workspace, or when replacing sherif, syncpack or knip.
license: MIT
---

# skif

skif is one binary that does the jobs of sherif (manifest and workspace rules), syncpack (version, range and catalog
policies) and knip (unused dependencies, files and exports). It reads code and configs statically and never runs the
project's code. A lint or fix run never uses the network; `skif update` is the only command that asks a registry.

Docs: <https://goldziher.github.io/skif/>. Rules: <https://goldziher.github.io/skif/rules/>.

## Run it

Use `skif` when it is on `PATH` (`skif --version`). Otherwise run the npm package, `skif-cli`, whose command is
`skif`: `npx skif-cli`, `pnpm dlx skif-cli`, `yarn dlx skif-cli` or `bunx -p skif-cli skif`. The npm package has no
Windows addon, so on Windows use a release binary (archive, `cargo binstall skif-cli` or Scoop). If the repository
pins skif (a `skif-cli` dev dependency, the GitHub Action, a pre-commit hook), use that version. Other install
channels are in [references/commands.md](references/commands.md#install).

The skif plugin also registers skif's MCP server, `skif`, which runs `npx -y skif-cli@<release> mcp`. When its
tools `lint`, `explain_usage` and `list_rules` are available, use them to lint, explain and list rules: they return
the documents the matching commands print with `--format json` (see
[references/json-output.md](references/json-output.md)). The server never writes files: run `skif --fix` to fix.

## The loop

1. **Lint as data.** Run `skif --format json` in the project (`-r` to cover every workspace and standalone package
   below the directory). stdout holds exactly one JSON document, also on failure. Read it as described in
   [references/json-output.md](references/json-output.md).
2. **Check the exit code.** `0` clean, `1` problems remain, `2` skif could not run (bad configuration, no
   `package.json`, bad flags, a refused fix). On `2` read the `error` document and fix its cause first.
3. **Triage.** Decide per finding: fix it, or ignore it with a reason. Follow
   [references/triage.md](references/triage.md); do not silence a finding you have not understood.
4. **Preview fixes.** `skif --fix --dry-run` prints the planned edits as a diff and writes nothing.
5. **Fix.** `skif --fix` applies safe fixes, lints again and runs the package manager's install (`--no-install`
   skips it). `--fix-unsafe` adds unsafe fixes (major moves, removing unused exports and dependencies); file
   deletion (`unused-files`) also needs `--allow-remove-files`. `--fix-type <rule or knip issue type>` narrows a run.
   Ask the user before `--fix-unsafe` or `--allow-remove-files` unless they asked for it.
6. **Verify.** Run `skif --format json` again and confirm exit `0`, or only the findings the user accepted.

`--fix` and `--fix-unsafe` are refused (exit `2`) when `CI` is set to a truthy value; `--dry-run` is allowed.

## Commands

| Command | Use it to |
| --- | --- |
| `skif [PATH]` | Lint the project containing `PATH` (default `.`). |
| `skif rules --format json` | List every rule ID with its category, default level and fixability. |
| `skif explain-usage <FILE[#EXPORT]>` | Learn why a file or export counts as used before deleting anything. |
| `skif --trace-dependency <NAME>` | See where a dependency is listed, imported and counted as used. |
| `skif config --show` / `--explain <KEY>` | See which `skif.toml` files apply and where a value was set. |
| `skif migrate --dry-run` | Translate sherif, syncpack and knip configuration into a `skif.toml`. |
| `skif baseline write\|prune\|check` | Record current findings so CI only accepts fewer. |
| `skif init` / `skif schema` | Write a starter `skif.toml` / print its JSON Schema. |
| `skif update --check` | List newer published versions (network). |
| `skif format --check` | Check `package.json` layout. |
| `skif mcp` / `skif lsp` | Run the MCP server for agents / the language server for editors. |

Every flag, with what it does: [references/commands.md](references/commands.md).

## Configuration

skif reads `skif.toml` (or `.skif.toml`) and layers the files from the project up to the git root; flags override
files. Keys are kebab-case and match rule IDs; unknown keys are errors naming the file and line. Do not guess a key:
check the JSON Schema (`skif schema`, or `schema/skif.schema.json` in the skif repository) and the
[configuration reference](https://goldziher.github.io/skif/reference/configuration/). After editing, run
`skif config --show` to see the merged result. Migrating from sherif, syncpack or knip, and adopting skif on a
repository with existing findings: [references/migrate.md](references/migrate.md).

## CI and hooks

The GitHub Action, pre-commit, prek and Poly hooks, report formats (SARIF, GitHub annotations, GitLab Code Quality,
Markdown) and the baseline ratchet: [references/ci.md](references/ci.md).
