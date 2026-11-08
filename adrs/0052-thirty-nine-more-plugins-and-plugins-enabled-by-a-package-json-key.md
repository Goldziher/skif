---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0051"]
---

# Thirty-Nine More Plugins and Plugins Enabled by a package.json Key

## Context and Problem Statement

The plugin table holds 161 plugins ([ADR 0051](0051-thirty-five-more-plugins-frameworks-task-runners-and-options-written-anywhere.md)). Issue #41 lists the third tier of knip plugins: Astro integrations (`astro-db`, `astro-markdoc`, `astro-og-canvas`, `expressive-code`, `lunaria`), Vite plugins and routers (`filesystem-routing`, `laravel-vite-plugin`, `solidjs-vite-plugin`, `vite-plugin-pages`, `vite-plugin-vue-layouts-next`, `vite-plus`, `vite-pwa-assets-generator`), test runners and story tools (`borp`, `playwright-test`, `ladle`, `react-cosmos`, `lost-pixel`), linters and release tools (`linthtml`, `lockfile-lint`, `mdxlint`, `npm-package-json-lint`, `bumpp`, `changelogithub`), platforms and agents (`railway`, `raycast`, `osls`, `eve`, `flue`, `mcp`, `openclaw`) and smaller ones (`catalyst`, `fast`, `create-typescript-app`, `kysely-codegen`, `marko`, `node-modules-inspector`, `tsrs`, `typescript-content-mapper`, `unplugin-vue-markdown`, `varlock`, `vercel-og`, `yorkie`).

Two of them, `openclaw` and `typescript-content-mapper`, are not enabled by a dependency or a file but by a key of the workspace's `package.json`, which no enabler of the table expresses. Others read a configuration format of their own: Varlock's `@plugin(…)` and `@import(…)` decorators sit in the comments of `.env` files, and MCP clients name the command of every server in `.mcp.json`, `.cursor/mcp.json`, `.vscode/mcp.json` or `.codex/config.toml`.

Which plugins are added, how is a plugin enabled by a `package.json` key, and where does skif deliberately not follow knip?

## Decision Drivers

* A plugin follows knip 6.44.1's enablers, config files, entry and production patterns and config-to-dependency resolution ([ADR 0036](0036-thirty-nine-more-knip-plugins-by-real-world-usage.md))
* Configs and sources are read statically and never executed ([ADR 0010](0010-js-analysis-on-oxc.md))
* A mechanism shared by several plugins is better than a parser per plugin ([ADR 0050](0050-twenty-one-more-plugins-and-commands-run-from-source.md))
* Every plugin has a fixture test, and the plugin lists in the documentation are checked against the code

## Considered Options

* Add the plugins that contribute something, with an enabler for a `package.json` key
* Add every listed plugin, including those whose knip counterpart only compiles files or marks class members
* Enable the two `package.json` key plugins by their tools' dependencies instead

## Decision Outcome

Chosen option: "Add the plugins that contribute something, with an enabler for a `package.json` key", because every added row changes what skif reports and the new enabler is the condition knip uses.

The added plugins are `astro-db`, `astro-og-canvas`, `bumpp`, `changelogithub`, `create-typescript-app`, `eve`, `expressive-code`, `kysely-codegen`, `lockfile-lint`, `lost-pixel`, `node-modules-inspector`, `vercel-og`, `vite-pwa-assets-generator`, `astro-markdoc`, `borp`, `ladle`, `linthtml`, `lunaria`, `marko`, `mcp`, `mdxlint`, `npm-package-json-lint`, `osls`, `playwright-test`, `raycast`, `react-cosmos`, `tsrs`, `varlock`, `yorkie`, `filesystem-routing`, `flue`, `laravel-vite-plugin`, `railway`, `solidjs-vite-plugin`, `vite-plugin-pages`, `vite-plugin-vue-layouts-next`, `vite-plus`, `openclaw` and `typescript-content-mapper`. The table holds 200 plugins.

`Enabler::ManifestKey` turns a plugin on when the workspace's `package.json` has a key, nested keys separated by `.` (`typescript.contentMapper`). The Vite plugins read their options from the plugin calls of `vite.config.*` with `extract_imported_calls` of [ADR 0050](0050-twenty-one-more-plugins-and-commands-run-from-source.md); `flue` and `vite-plus` read theirs wherever the config writes them with `extract_property_values` of [ADR 0051](0051-thirty-five-more-plugins-frameworks-task-runners-and-options-written-anywhere.md). A config passed with `kysely-codegen --config-file` or `railway --file` is a config file, and `tsrs -p` reads a project as `tsc -p` does.

Not added:

* `catalyst` and `fast` mark the classes of custom elements as used through a source visitor and contribute nothing else; skif does not mark custom elements ([ADR 0051](0051-thirty-five-more-plugins-frameworks-task-runners-and-options-written-anywhere.md) records the same for `stencil`), so a row would only be an enabler.
* `unplugin-vue-markdown` registers a compiler that finds the auto-imported components of `.md` files and has no enabler-independent content; skif compiles no Markdown, so a row would change nothing.

Where skif differs from knip:

* `marko` does not compile `.marko` files and does not read the tag libraries of installed packages; it credits the `marko` package when a `.marko` file exists, as knip's compiler does through the `import "marko"` it adds to every template.
* `varlock` does not read `~/` paths, does not skip git-ignored `.env` files and does not report an `@import` of a missing file; it reads `--path` in scripts whatever the `--env` flag. A `@plugin` package keeps its name without an `@version`, and a relative `@plugin` or `@import` outside the workspace is not followed.
* `railway` reads the `service(…)` calls of `.railway/railway.ts`, but a `source` written as a call (`github("acme/app")`, `image("postgres")`) is not known statically and counts as local, and only `package.json#repository` names the local repository, not the git remotes. A bare `owner/name` is GitHub shorthand even when it contains "github"; only a `github.com` host or a `github:` prefix is cut off. A `rootDirectory` is relative to the repository root, with a leading `/` cut off; one with `..` drops the service's commands.
* `mcp` quotes a server argument holding whitespace when it joins the command, as the command after `varlock run --` is joined.
* `openclaw` makes the files of its `assetScripts` entries, not production entries.
* `solidjs-vite-plugin`, `filesystem-routing`, `vite-plugin-pages`, `vite-plugin-vue-layouts-next` and `flue` keep their conventional production entries when a config is read, as the other plugins of the table do; knip replaces them.
* `mdxlint` keeps a scoped plugin name as it is, where knip prefixes it with `remark-`.
* `ladle` reads its `viteConfig` with the `vitest` plugin, as knip does, through a linked config.

### Consequences

* Good, because the false findings of 39 more tools disappear without running any tool, among them the commands of MCP servers and the plugins of Varlock `.env` schemas.
* Good, because a plugin enabled by a `package.json` key needs no plugin-specific detection.
* Neutral, because the two plugins left out remain knip features without a skif counterpart.
* Bad, because a Railway service whose source is a call, or a Varlock schema outside the load paths, is read imprecisely.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs` lists the 200 ids in `ADR_IDS` and has a fixture test for each new plugin.
* `crates/skif-js-plugins/src/plugins/varlock.rs` tests the decorator parser, `railway.rs` the repository names and `script_config.rs` the command after `varlock run --`.
* `scripts/check-plugin-list.sh` fails when the plugin lists or counts in the README files and the website differ from the code.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Add the plugins that contribute something, with an enabler for a `package.json` key

* Good, because every row changes what is reported.
* Bad, because the enabler enum grows by one variant.

### Add every listed plugin, including those whose knip counterpart only compiles files or marks class members

* Good, because the table would list every name of the issue.
* Bad, because three rows would be enablers without any effect, which reads as support that does not exist.

### Enable the two `package.json` key plugins by their tools' dependencies instead

* Good, because no enabler is added.
* Bad, because neither tool has a dependency that marks a package using it: an OpenClaw plugin and a package with a content mapper declare themselves only in `package.json`.
