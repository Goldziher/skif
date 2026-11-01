---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Plugin Model: A Declarative Table With Optional AST Resolvers

## Context and Problem Statement

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
small fraction of repositories. [ADR 0010](0010-js-analysis-on-oxc.md) forbids executing configs, so `resolveConfig` has no direct
equivalent.

## Decision Drivers

* Accurate JS analysis depends on knowing about tools
* Matching all 202 knip plugins before shipping would delay JS analysis indefinitely
* Configs must not be executed ([ADR 0010](0010-js-analysis-on-oxc.md)), so `resolveConfig` has no direct equivalent

## Considered Options

* A declarative plugin table with optional AST config resolvers
* Port all of knip's plugins before release
* Plugins as dynamically loaded libraries or WASM
* User-defined plugins in `skif.toml`

## Decision Outcome

Chosen option: "A declarative plugin table with optional AST config resolvers", because most plugins are a few lines of data, reviewable without reading code and free at runtime unless enabled, while the resolver hook covers config-driven plugins without executing anything.

A skif plugin is a **row in a declarative table** in `skif-js-plugins`, with an optional AST
resolver, and is exposed through one trait:

```rust
pub trait Plugin: Send + Sync {
    /// The declarative row: enablers, config, entry, production and project patterns.
    fn spec(&self) -> &'static PluginSpec;

    /// Reads the plugin's parsed config file and returns what it references. The default
    /// returns nothing, which is all a purely declarative plugin needs.
    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        Vec::new()
    }
}

pub struct PluginSpec {
    pub id: &'static str,                 // "vitest"
    pub enablers: &'static [Enabler],     // dependency names or regexes
    pub root_only: bool,
    pub config: &'static [&'static str],  // config file globs
    pub entry: &'static [&'static str],   // entry globs
    pub production: &'static [&'static str],
    pub project: &'static [&'static str],
    pub package_json_key: Option<&'static str>,
}

pub enum PluginInput {                    // #[non_exhaustive]
    Entry(String), Dependency(String), Binary(String), ConfigFile(String),
}
```

The table is a `&'static [&'static dyn Plugin]`; a declarative plugin is a unit struct that
implements `spec` only. `ConfigAst` is skif's view of the parsed config ([ADR 0010](0010-js-analysis-on-oxc.md)); it holds no oxc
type ([ADR 0003](0003-library-first-embedding.md)).

* A plugin is **enabled** for a workspace when an enabler matches one of its dependencies, or when
  `[js.plugins] <id> = true` forces it; `<id> = false` disables it.
* `config`, `entry`, `production` and `project` patterns apply without any code.
* `Plugin::resolve_config` receives the parsed config file ([ADR 0010](0010-js-analysis-on-oxc.md)'s static extraction, with constants
  inlined) and returns `PluginInput`s: extra entries, dependencies, binaries, and further config
  files. It never sees an evaluated module. It corresponds to knip's `resolveFromAST`, and replaces
  `resolveConfig` where the information is statically reachable.
* Users override any plugin's patterns per workspace in `[js.workspaces."<dir>"]` ([ADR 0004](0004-configuration-model.md)).

The **first set of 21** — the plugins 0.2.0 ships, each with at least one fixture — is the twenty
most common tools plus the CI workflow reader: typescript, eslint, prettier, vite, vitest, jest,
next, storybook, playwright, tailwind, postcss, babel, webpack, rollup, tsup, turbo, husky,
lint-staged, commitlint, changesets, and github-actions. All 21 exist in knip under the same
names (checked at commit `da07b6e9887f9a1eb08545da29755b7f9d309bca`), which keeps the parity
comparison direct.

A plugin is **added** when:

1. it is enabled in at least one corpus repository, or a user reports a false positive it would
   fix;
2. it ships with a fixture that fails without it and passes with it;
3. its config can be read statically, or its patterns alone remove the false positives.

**Parity with knip is a metric, not a gate.** `mise run parity` reports, per corpus repository,
the issues knip finds that skif does not and the reverse. Releases are not blocked on a plugin
count; they are blocked on regressions in that diff.

### Consequences

* Good, because most plugins are a few lines of data, reviewable without reading code, and cost nothing at
  runtime unless enabled.
* Good, because the resolver hook covers the plugins whose value is in their config (Vitest setup files, ESLint
  plugins and parsers) without executing anything.
* Good, because a user can patch a missing plugin's effect with `entry` and `ignore-dependencies` until a plugin
  lands.
* Bad, because twenty-one plugins against 202 means real repositories will see false positives that knip would
  not report. The fixed first list targets the most common tools; `[js] ignore-dependencies`,
  `ignore-binaries` and `entry` are the escape hatches.
* Bad, because plugins whose knowledge only exists in the evaluated config cannot be ported faithfully; they
  degrade to their static patterns.
* Bad, because knip adds and changes plugins continuously. The table records the knip commit each plugin was
  checked against, so drift is visible.

### Confirmation

* A `skif-js-plugins` test asserts that the plugin table contains exactly the 21 IDs listed above, that IDs are unique, and that every plugin has a fixture directory `fixtures/js/plugins/<id>/`.
* Each plugin's fixture test fails without the plugin and passes with it: the test runs the fixture with `[js.plugins] <id> = false` and expects the false positives, then with the plugin enabled and expects none.
* A test asserts that enabling and disabling through `[js.plugins]` overrides the enabler match, and that a per-workspace `[js.workspaces."<dir>"]` pattern replaces the plugin's default.
* A compile-time assertion covers `Send + Sync` for `Plugin` implementors, and a rustdoc-JSON check covers that `ConfigAst` exposes no oxc type.
* `mise run parity` reports the per-repository diff against knip; a release is blocked on a regression of that diff against the committed baseline, not on a plugin count.
* The plugin table stores the knip commit each plugin was checked against, and a `consistency` job fails when a row lacks one.

## Pros and Cons of the Options

### A declarative plugin table with optional AST config resolvers

* Good, because most plugins are a few lines of data, reviewable without reading code, and cost nothing at runtime unless enabled; the resolver hook covers config-driven plugins without executing anything.
* Bad, because 21 plugins against 202 means real repositories see false positives knip would not report, and plugins whose knowledge only exists in an evaluated config degrade to their static patterns.

### Port all of knip's plugins before release

* Good, because parity would be as close as the semantics allow on day one.
* Bad, because it means months of work for a long tail of tools few repositories use, before anyone can use the analysis.

### Plugins as dynamically loaded libraries or WASM

* Good, because third parties could add plugins without a skif release.
* Bad, because a plugin ABI is a large public surface for a table that is mostly static data; contributions go upstream instead.

### User-defined plugins in `skif.toml`

* Good, because users could cover their own tools without waiting for a release.
* Bad, because a declarative user format is a second plugin definition to design and support while the per-workspace `entry`, `project` and ignore options already cover the need. (deferred)

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0010](0010-js-analysis-on-oxc.md)
* Upstream: [knip](https://github.com/webpro-nl/knip)
* See the [ADR index](index.md).
