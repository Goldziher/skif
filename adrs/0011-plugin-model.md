# 0011 — Plugin Model: A Declarative Table With Optional AST Resolvers

* Status: Accepted
* Date: 2026-11-01

## Context

Most of what makes JS analysis accurate is knowing about tools. `vitest` in `devDependencies` is
used if a `vitest.config.ts` exists, `*.test.ts` files are entries rather than unused files, and
the reporter named in that config is a dependency even though no source file imports it. knip
captures this knowledge in plugins.

At commit `da07b6e9887f9a1eb08545da29755b7f9d309bca`, knip has **202 plugins**
(`packages/knip/src/plugins`, excluding `_`-prefixed helpers). Its `Plugin` interface
(`types/config.ts`) is mostly data — `title`, `enablers`, `isEnabled`, `isRootOnly`, `config`,
`entry`, `production`, `project`, `packageJsonPath`, `args` — plus behaviour hooks: `resolveConfig`
(116 plugins, receives the *executed* config module), `resolveFromAST` (30 plugins), `resolve`,
`resolveSourceMap`, `registerCompilers` and `registerVisitors`.

Matching all 202 before shipping would delay JS analysis indefinitely, and most of them serve a
small fraction of repositories. ADR 0010 forbids executing configs, so `resolveConfig` has no direct
equivalent.

## Decision

A skif plugin is a **row in a declarative table** in `skif-js-plugins`, with an optional AST
resolver:

```rust
pub struct PluginSpec {
    pub id: &'static str,                 // "vitest"
    pub enablers: &'static [Enabler],     // dependency names or regexes
    pub root_only: bool,
    pub config: &'static [&'static str],  // config file globs
    pub entry: &'static [&'static str],   // entry globs
    pub production: &'static [&'static str],
    pub project: &'static [&'static str],
    pub package_json_key: Option<&'static str>,
    pub resolver: Option<&'static dyn ConfigResolver>,
}

pub trait ConfigResolver: Send + Sync {
    fn resolve(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput>;
}
```

* A plugin is **enabled** for a workspace when an enabler matches one of its dependencies, or when
  `[js.plugins] <id> = true` forces it; `<id> = false` disables it.
* `config`, `entry`, `production` and `project` patterns apply without any code.
* A `ConfigResolver` receives the parsed config file (ADR 0010's static extraction, with constants
  inlined) and returns `PluginInput`s: extra entries, dependencies, binaries, and further config
  files. It never sees an evaluated module. It corresponds to knip's `resolveFromAST`, and replaces
  `resolveConfig` where the information is statically reachable.
* Users override any plugin's patterns per workspace in `[js.workspaces."<dir>"]` (ADR 0004).

The **first set** — the plugins 0.2.0 ships, each with at least one fixture — is:
typescript, eslint, prettier, vite, vitest, jest, next, storybook, playwright, tailwind, postcss,
babel, webpack, rollup, tsup, turbo, husky, lint-staged, commitlint, changesets, github-actions.
All exist in knip under the same names, which keeps the parity comparison direct.

A plugin is **added** when:

1. it is enabled in at least one corpus repository, or a user reports a false positive it would
   fix;
2. it ships with a fixture that fails without it and passes with it;
3. its config can be read statically, or its patterns alone remove the false positives.

**Parity with knip is a metric, not a gate.** `mise run parity` reports, per corpus repository,
the issues knip finds that skif does not and the reverse. Releases are not blocked on a plugin
count; they are blocked on regressions in that diff.

## Consequences

Positive:

* Most plugins are a few lines of data, reviewable without reading code, and cost nothing at
  runtime unless enabled.
* The resolver hook covers the plugins whose value is in their config (Vitest setup files, ESLint
  plugins and parsers) without executing anything.
* A user can patch a missing plugin's effect with `entry` and `ignore-dependencies` until a plugin
  lands.

Negative / risks:

* Twenty-one plugins against 202 means real repositories will see false positives that knip would
  not report. The fixed first list targets the most common tools; `[js] ignore-dependencies`,
  `ignore-binaries` and `entry` are the escape hatches.
* Plugins whose knowledge only exists in the evaluated config cannot be ported faithfully; they
  degrade to their static patterns.
* knip adds and changes plugins continuously. The table records the knip commit each plugin was
  checked against, so drift is visible.

## Alternatives considered

* **Port all of knip's plugins before release:** rejected — months of work for a long tail of
  tools few repositories use, before anyone can use the analysis.
* **Plugins as dynamically loaded libraries or WASM:** rejected — a plugin ABI is a large public
  surface for a table that is mostly static data; contributions go upstream instead.
* **User-defined plugins in `skif.toml`:** deferred — the per-workspace `entry`, `project` and
  ignore options cover the need until a declarative user plugin format proves necessary.
