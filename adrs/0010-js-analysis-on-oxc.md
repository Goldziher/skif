---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# JS Analysis on oxc: knip Semantics, Static Configs, Three Crates

## Context and Problem Statement

Manifest rules and policies ([ADR 0009](0009-version-and-catalog-policies.md)) read only `package.json` files. The questions users ask
next — which listed dependencies are never imported, which imports are not listed, which files and
exports nothing uses — require parsing the source and building a module graph.

knip answers them and is the reference for what the answers should be. Read at commit
`da07b6e9887f9a1eb08545da29755b7f9d309bca` (knip 6.44.1, ISC), its pipeline is:

1. **Workspaces and configuration** (`ConfigurationChief`): find workspaces, merge `knip.json` with
   defaults, and per workspace decide which plugins are enabled ([ADR 0011](0011-plugin-model.md)).
2. **Entry and project files**: each plugin contributes entry patterns (test files, config files,
   framework routes) and reads tool configuration to find more (a Vitest `setupFiles`, an ESLint
   plugin). `package.json` `main`, `exports`, `bin` and scripts add entries; scripts are parsed to
   find binaries and the files they run.
3. **Graph** (`graph/build.ts`): starting from entries, parse each file (knip uses `oxc-parser`
   0.155 through Node bindings), collect imports, re-exports, `require`, dynamic `import()`, and
   exports with their members; resolve specifiers with `oxc-resolver` 11.24.2 against tsconfig paths
   and `exports` conditions; follow until no new files appear.
4. **Analysis** (`graph/analyze.ts`, `DependencyDeputy`): mark referenced exports (including through
   namespaces and re-export chains), honour JSDoc tags such as `@public` and `@internal`, and compare
   referenced packages and binaries against each workspace's manifest.
5. **Issues**, one per type: `files`, `dependencies`, `devDependencies`,
   `optionalPeerDependencies`, `unlisted`, `binaries`, `unresolved`, `exports`, `nsExports`,
   `types`, `nsTypes`, `enumMembers`, `namespaceMembers`, `duplicates`, `catalog`,
   `catalogReferences`, `cycles`.

**Production mode** (`--production`) restricts entries to the ones plugins mark as production,
excludes `devDependencies` and test files, and so answers "what does shipped code use".

Two properties of knip's design are ones skif will not copy. First, knip **loads tool configuration
by executing it**: 116 of its 202 plugins implement `resolveConfig`, which receives the evaluated
config module (TypeScript configs run through `jiti`), and only 30 also have a `resolveFromAST`
path. Executing a repository's `vite.config.ts` to lint it runs arbitrary code with the user's
privileges. Second, it runs in Node, so every parsed file crosses a Node binding boundary and every
run pays Node start-up and module loading.

fallow (fallow-rs/fallow, MIT) is an existing Rust implementation of similar analysis on oxc. It
shows the approach is viable and is a useful design reference.

## Decision Drivers

* Answer which dependencies are unused or unlisted and which files and exports nothing uses
* Never execute a repository's configuration, since that runs arbitrary code with the user's privileges
* Meet the performance targets of [ADR 0001](0001-mission-and-scope.md) without Node start-up and binding costs
* oxc publishes breaking minor releases roughly weekly, and no oxc type may escape ([ADR 0003](0003-library-first-embedding.md))

## Considered Options

* Native knip-semantics analysis on exactly pinned oxc crates, with static config extraction, in three crates
* Run knip and translate its JSON
* The TypeScript compiler API or `swc`
* Evaluate configs in an embedded JS engine (QuickJS, Boa)
* Build on fallow

## Decision Outcome

Chosen option: "Native knip-semantics analysis on exactly pinned oxc crates, with static config extraction, in three crates", because it removes Node, the binding boundary and config execution, so analysis runs in one process on rayon, is safe on untrusted pull requests and confines oxc upgrades to three crates.

skif implements knip's semantics natively on **oxc crates**, in three crates and nowhere else
([ADR 0002](0002-crate-layout-and-publishing.md)):

* **`skif-js-syntax`** — the only crate that names oxc parser types. Per file: `oxc_parser` with the
  `ModuleRecord` for static imports and exports, plus a custom `Visit` pass for what the record does
  not hold: `require()`, dynamic `import()` with constant arguments, `import.meta.glob`, JSDoc tags,
  namespace and enum members, and byte ranges for removal fixes. `oxc_semantic` resolves in-file
  references, so an export that is used only inside its own file is detected correctly. Output is a
  skif-owned `ModuleInfo`; no oxc type escapes ([ADR 0003](0003-library-first-embedding.md)). It also provides **constant inlining**
  over a config file's AST for plugins.
* **`skif-js-plugins`** — the plugin table and AST config resolvers ([ADR 0011](0011-plugin-model.md)), and the parser for
  `package.json` scripts that turns `"test": "vitest run --config vitest.ci.ts"` into a binary
  (`vitest`) and an entry file.
* **`skif-js`** — the module graph and the JS rules. Resolution uses **`oxc_resolver` 11.24.x** with
  tsconfig `paths` and project references and `exports`/`imports` conditions; the graph is built by a
  worklist from entries, parses files in parallel, and keeps `imported_by` maps per module and export.
  The JS rules ([ADR 0007](0007-rule-and-diagnostic-model.md)) are exposed as a `RuleProvider`.

Issue types map to rule IDs one to one: `files` → `unused-files`, `dependencies` →
`unused-dependencies`, `devDependencies` → `unused-dev-dependencies`, `optionalPeerDependencies` →
`unused-optional-peer-dependencies`, `unlisted` → `unlisted-dependencies`, `binaries` →
`unlisted-binaries`, `unresolved` → `unresolved-imports`, `exports` → `unused-exports`, `nsExports`
→ `unused-namespace-exports`, `types` → `unused-types`, `nsTypes` → `unused-namespace-types`,
`enumMembers` → `unused-enum-members`, `namespaceMembers` → `unused-namespace-members`,
`duplicates` → `duplicate-exports`, `cycles` → `circular-imports`. knip's `catalog` and
`catalogReferences` are already covered by the policy rules `unused-catalog-entries` and
`unresolved-catalog-references` ([ADR 0009](0009-version-and-catalog-policies.md)) and are not duplicated.

`[js] production = true` (or `--production`) implements production mode with knip's meaning.

Binding constraints:

* **Every `oxc_*` crate is pinned exactly** (`= 0.156.0` at the time of writing) in
  `[workspace.dependencies]`, and dependabot groups all oxc crates into one pull request. oxc
  publishes breaking minor releases roughly weekly; a partial upgrade does not compile.
* **Configuration is read by static AST extraction only.** skif parses `vite.config.ts` and walks
  the default export — object literals, `defineConfig(...)` wrappers, string and array constants
  inlined from the same file — and never evaluates it. Anything computed at runtime is reported as a
  hint that the config could not be fully read, not guessed.
* **knip is the specification; its code is not copied.** Its documentation, issue types and
  observed output on the corpus define correct behaviour; test fixtures may be adapted from knip
  under its ISC license with attribution ([ADR 0015](0015-attribution-and-vendoring.md)).
* **fallow is a design reference only.** No fallow code is copied or adapted.
* JS analysis is **off by default in the library** (`js-analysis` feature, [ADR 0003](0003-library-first-embedding.md)) and enabled in
  the CLI; it ships in 0.2.0 ([ADR 0001](0001-mission-and-scope.md)).

### Consequences

* Good, because no Node, no binding boundary, no config execution: parsing, resolution and analysis run in one
  process on rayon, which is where the ≥ 5× target over knip and the 10,000-file budget come from
  ([ADR 0001](0001-mission-and-scope.md)). The parser and resolver are the same oxc projects knip itself uses, so syntax
  coverage and resolution behaviour start aligned.
* Good, because running skif on an untrusted pull request is safe: it reads files and nothing else.
* Good, because oxc upgrades touch three crates; the rest of skif does not change when oxc does.
* Bad, because static extraction misses configuration built at runtime (`process.env` branches, imported
  helpers, async config functions). Those cases produce a hint and may produce false
  `unused-files` until the user adds `entry` patterns. Measured on the corpus by `mise run parity`.
* Bad, because exact oxc pins conflict with any host that uses a different oxc (poly pins a git revision); such a
  host leaves `js-analysis` off or aligns its pin. This is the main reason the feature is opt-in.
* Bad, because matching knip's semantics without its code means its edge cases are rediscovered through the
  corpus diff, not inherited.

### Confirmation

* `mise run parity` reports per corpus repository the issues knip finds that skif does not and the reverse.
* `mise run bench` measures the 5x target over knip and the 10,000-file budget.
* Every `oxc_*` crate is pinned exactly in `[workspace.dependencies]` and dependabot groups all oxc crates into one pull request.

## Pros and Cons of the Options

### Native knip-semantics analysis on exactly pinned oxc crates, with static config extraction, in three crates

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Run knip and translate its JSON

* Bad, because it needs Node, config execution and none of the performance targets.

### The TypeScript compiler API or `swc`

* Bad, because knip itself already relies on oxc's parser and resolver, the sibling tools (poly) already build on oxc, and oxc's `ModuleRecord`, semantic analysis and resolver cover the pipeline without a type checker.

### Evaluate configs in an embedded JS engine (QuickJS, Boa)

* Bad, because it still executes user code, cannot load the `node_modules` imports most configs start with, and adds a large dependency.

### Build on fallow

* Bad, because skif's crate boundaries, diagnostic model and embedding contract ([ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md)) differ enough that adopting it would mean rewriting it; it is read, not reused.

## More Information

* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0011](0011-plugin-model.md), [ADR 0015](0015-attribution-and-vendoring.md)
* Upstream: [knip](https://github.com/webpro-nl/knip)
* Upstream: [fallow](https://github.com/fallow-rs/fallow)
* Upstream: [oxc](https://github.com/oxc-project/oxc)
* See the [ADR index](index.md).
