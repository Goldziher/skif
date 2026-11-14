# Triaging skif findings

Work from the JSON report (`skif --format json`). Group the diagnostics by `rule`, read the rule's section at
`https://github.com/Goldziher/skif/blob/main/docs/rules.md#<rule-id>` when its meaning is not obvious, and handle one
group at a time.
For each finding the outcome is one of: fixed, ignored with a written reason, or left for the user with an
explanation. Never delete code, files or dependencies on a guess.

## Manifest, workspace and policy rules

These come from the manifests, `pnpm-workspace.yaml` and the version policies (the sherif and syncpack side, and what
`--no-js` still runs). Most have fixes:

1. Preview with `skif --fix --dry-run --no-js` and read the diff.
2. Apply with `skif --fix` (add `--no-install` when the user will install).
3. When a finding has several `fixes` (a version mismatch with several candidate versions), choose with
   `--select highest|lowest`, set `[policy] prefer` in `skif.toml`, or ask the user. `--no-prompt` never asks.

A version finding the user wants to keep is a policy decision, not a suppression: a `[[version-groups]]` entry (pin a
version, or `policy = "ignored"` for a group) or a `[[semver-groups]]` range policy. Catalog rules
(`catalog-mismatch`, `catalog-missing-entry`, `catalog-not-used`, `unused-catalog-entries`) follow the `[catalog]`
section. See the [configuration reference](https://goldziher.github.io/skif/reference/configuration/).

## Unused dependencies

`unused-dependencies`, `unused-dev-dependencies` and `unused-optional-peer-dependencies` mean no import, script,
binary use or tool configuration that skif reads uses the package.

- Confirm with `skif --trace-dependency <name> --format json`: where it is listed, every import, and which packages
  count it as used.
- A package used only by a tool whose config skif cannot read statically, or loaded by a string at run time, is a
  false positive: add it to `ignore-dependencies` of the package's `[js.workspaces."<dir>"]` table with a reason.
- In a repository with several roots (nested workspaces or standalone packages), lint with `-r` before removing:
  a single-root run does not see the code of the roots below it.
- Check `incomplete-packages`: there unused dependencies are not reported at all, so the absence of a finding there
  proves nothing.
- Remove with `skif --fix-unsafe --fix-type dependencies`, then install.

## Unlisted dependencies, binaries and imports

- `unlisted-dependencies`: a package is imported but its manifest does not list it. Add it to the importing
  package's `dependencies` (production code) or `devDependencies` (tests, configs, scripts), with the version the
  workspace already uses. Under `--strict`, production code must import from `dependencies` or `peerDependencies`.
- `unlisted-binaries`: a script, workflow command or tool config runs a binary no listed dependency provides. List the package, or ignore a system
  binary with `[js] ignore-binaries`.
- `unresolved-imports`: an import that resolves to no file. Check path aliases (`tsconfig.json` `paths`, or
  `paths` in `[js.workspaces."<dir>"]`), generated files and virtual modules; list a deliberate one in
  `[js] ignore-unresolved`.

## Unused files

`unused-files` means no entry file reaches the file through imports.

1. Run `skif explain-usage <file>`: an `absence` of `unused-file` confirms it; other values (`ignored`,
   `not-discovered`, `entries-incomplete`) explain why the file is out of the graph.
2. If the file is an entry skif does not know (a script, a tool config, a framework convention), add it to `[js]
   entry` (or the package's `[js.workspaces."<dir>"] entry`) or enable the tool's plugin in `[js.plugins]`, rather
   than ignoring it.
3. Delete only with the user's consent: `skif --fix-unsafe --fix-type files --allow-remove-files`, or delete by hand.

## Unused subpath imports and exports

- `unused-subpath-imports`: no `#` specifier of the package's files selects the `imports` entry. A script or tool
  that imports it where skif does not read it is a false positive; otherwise remove the entry with
  `skif --fix-unsafe --fix-type unused-subpath-imports`.
- `unused-subpath-exports` (off by default): no workspace package imports the `exports` subpath by name. Code outside
  the workspace may import a published subpath, so remove one only when the package is private or the subpath is not
  public API.

## Unused exports, types and members

`unused-exports`, `unused-types`, `unused-enum-members`, `unused-namespace-*` and `duplicate-exports` are about the
module graph inside the project.

- Run `skif explain-usage <file>#<export>`; the `verdict` is `public`, `used`, `unused` or `not-judged`.
- An export that is public API of a published package is used from outside: tag it `@public` in its JSDoc, or add an
  `ignore-tags` option to `unused-exports` for a project-specific tag.
- Remove with `skif --fix-unsafe --fix-type exports,types` (also `enumMembers`, `namespaceMembers`). Some findings
  have no fix because removal could change behaviour; edit those by hand.

## Incomplete packages and stale configuration

- An `incomplete-packages` entry names the `cause` and the `file` that made the entry files unknown (`unknown-config`,
  `unknown-spread`, `computed-entries`, `unparsable-config`, `unmapped-published`, `unmapped-entry`, `no-entries`,
  ...). Making that config static, pointing a build-output entry at its source, or listing the entries in
  `[js] entry`, lets skif judge the package.
- `stale-js-config` reports configuration entries that matched nothing: remove them.
- `unparseable-files` reports a reachable file with a syntax error the parser could not recover from.

## Ignoring a finding

Use the narrowest form, and always write a `reason`:

```toml
[[ignore]]
rules = ["version-mismatch"]
packages = ["@acme/legacy"]
dependencies = ["react@^17.0.2"]   # name@specifier silences only that version
reason = "legacy app stays on React 17 until its rewrite"

[js.workspaces."packages/app"]
ignore-dependencies = [{ name = "some-loader", reason = "loaded by the bundler from a config string" }]
```

- `[[ignore]]` narrows by `rules`, `packages`, `dependencies` and `files` (at least one).
- `[rules] <id> = "off" | "info" | "warn" | "error"` changes a rule's level for the whole root;
  `[[overrides]]` does it for selected packages or files.
- In version files, Dockerfiles, workflows and shell scripts, `# skif-ignore-next-line <rule> -- <reason>` and
  `# skif-ignore-file <rule> -- <reason>` suppress `toolchain-pin-drift` and `foreign-package-manager-command`; the
  reason is required.
- `--require-ignore-reasons` (or `[lint] require-ignore-reasons = true`) fails a run on an entry without a reason.
- Too many findings to handle now: `skif baseline write` records them (see [migrate.md](migrate.md#adopting-skif)).
