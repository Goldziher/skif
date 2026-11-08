//! False-positive classes of the module graph: signature references, promise callbacks, comments, ambient modules,
//! duplicate exports, entries outside the declaring package, discovery and fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs, OsFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";
const PACKAGE: (&str, &str) = ("package.json", r#"{"name":"app","private":true}"#);

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            let name = d.dependency.clone().unwrap_or_else(|| {
                let start = d.message.find('`').map_or(0, |i| i + 1);
                let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
                d.message[start..end].to_owned()
            });
            format!("{}: {name}", d.location.path.display())
        })
        .collect();
    out.sort();
    out
}

fn none() -> Vec<String> {
    Vec::new()
}

#[test]
fn types_in_the_signature_of_a_used_export_are_used() {
    let diagnostics = lint(&[
        PACKAGE,
        (
            "src/index.ts",
            "import { Widget, make } from './widget';\nimport './orphan';\nmake();\nWidget({ a: '' });\n",
        ),
        (
            "src/widget.ts",
            "export interface WidgetProps { a: string }\n\
             export interface Made { b: number }\n\
             export interface Lonely { c: number }\n\
             export function Widget(p: WidgetProps): void {}\n\
             export const make = (): Made => ({ b: 1 });\n",
        ),
        (
            "src/orphan.ts",
            "export interface OrphanProps { a: string }\nexport function Orphan(p: OrphanProps): void {}\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-types"),
        ["src/orphan.ts: OrphanProps", "src/widget.ts: Lonely"]
    );
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/orphan.ts: Orphan"]);
}

#[test]
fn types_reached_through_a_chain_of_used_signatures_are_used() {
    let diagnostics = lint(&[
        PACKAGE,
        ("src/index.ts", "import { run } from './run';\nrun(null as never);\n"),
        (
            "src/run.ts",
            "export type Options = { x: number };\nexport function configure(o: Options): void {}\n\
             export function run(c: ReturnType<typeof configure>): void {}\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-types"), none());
    assert_eq!(found(&diagnostics, "unused-exports"), none());
}

#[test]
fn then_callbacks_read_the_exports_they_name() {
    let diagnostics = lint(&[
        PACKAGE,
        (
            "src/index.ts",
            "export const a = import('./page').then((m) => m.Page);\n\
             export const b = import('./lazy').then(({ Lazy }) => Lazy);\n\
             export const c = import('./wrapped').then((m) => ({ default: m.Inner }));\n",
        ),
        ("src/page.ts", "export const Page = 1;\nexport const PageOther = 2;\n"),
        ("src/lazy.ts", "export const Lazy = 1;\nexport const LazyOther = 2;\n"),
        (
            "src/wrapped.ts",
            "export const Inner = 1;\nexport const InnerOther = 2;\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        [
            "src/lazy.ts: LazyOther",
            "src/page.ts: PageOther",
            "src/wrapped.ts: InnerOther"
        ]
    );
}

#[test]
fn jsdoc_import_calls_in_prose_are_not_imports() {
    let diagnostics = lint(&[
        PACKAGE,
        (
            "src/index.ts",
            "/**\n * Wraps `lazy(() => import('./Missing'))` for the {@link import('./AlsoMissing')} case.\n * @type {import('./Present').Thing}\n */\nexport const wrapped = 1;\n",
        ),
        ("src/Present.ts", "export type Thing = string;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn declared_ambient_modules_are_not_unlisted() {
    let diagnostics = lint(&[
        PACKAGE,
        (
            "types/flags.d.ts",
            "declare module 'flags' {\n  export const enabled: boolean;\n}\n",
        ),
        (
            "src/index.ts",
            "import { enabled } from 'flags';\nimport left from 'left-pad';\nexport const on = [enabled, left];\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), ["src/index.ts: left-pad"]);
}

#[test]
fn clause_aliases_and_entry_aliases_are_not_duplicates() {
    let diagnostics = lint(&[
        PACKAGE,
        (
            "src/index.ts",
            "import { A, B, x } from './helpers';\nexport const h = 1;\nexport { h as GET };\nexport const used = [A, B, x];\n",
        ),
        (
            "src/helpers.ts",
            "const handler = 1;\nexport { handler as A, handler as B };\nexport const x = 1;\nexport default x;\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "duplicate-exports"), ["src/helpers.ts: x"]);
}

#[test]
fn entries_of_another_package_are_attributed_to_its_owner() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["pkg-a","lib-b"]}"#,
        ),
        (
            "pkg-a/package.json",
            r#"{"name":"pkg-a","private":true,"devDependencies":{"vitest":"3.0.0"}}"#,
        ),
        (
            "pkg-a/vitest.config.ts",
            "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { setupFiles: ['../lib-b/setup.ts'] } });\n",
        ),
        (
            "pkg-a/src/a.test.ts",
            "import { it } from 'vitest';\nit('x', () => {});\n",
        ),
        (
            "lib-b/package.json",
            r#"{"name":"lib-b","private":true,"devDependencies":{"dotenv":"16.0.0","vitest":"3.0.0"}}"#,
        ),
        (
            "lib-b/setup.ts",
            "import dotenv from 'dotenv';\nimport { beforeAll } from 'vitest';\nbeforeAll(() => dotenv.config());\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
}

#[test]
fn marker_manifests_do_not_hide_directories_but_packages_do() {
    let diagnostics = lint(&[
        PACKAGE,
        ("src/index.ts", "export {};\n"),
        ("scripts/package.json", r#"{"type":"module"}"#),
        ("scripts/run.js", "export const run = 1;\n"),
        ("vendor/package.json", r#"{"name":"vendored"}"#),
        ("vendor/lib.js", "export const lib = 1;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), ["scripts/run.js: scripts/run.js"]);
}

#[test]
fn fixture_directories_are_not_held_to_their_imports() {
    let diagnostics = lint(&[
        PACKAGE,
        (
            "src/index.ts",
            "import './missing';\nimport 'real-missing';\nimport './__fixtures__/data';\nexport {};\n",
        ),
        (
            "test/fixtures/broken.ts",
            "import './nope';\nimport 'fixture-only';\nexport {};\n",
        ),
        ("src/__fixtures__/data.ts", "import 'other-fixture';\nexport {};\n"),
    ]);
    assert_eq!(found(&diagnostics, "unresolved-imports"), ["src/index.ts: ./missing"]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["src/index.ts: real-missing"]
    );
}

#[test]
fn a_listed_package_reached_through_a_path_alias_is_used() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","dependencies":{"@scope/dep":"1.0.0","other":"1.0.0"}}"#,
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@scope/*":["vendor/*/src"]}}}"#,
        ),
        ("src/index.ts", "import { a } from '@scope/dep';\na();\n"),
        ("vendor/dep/src/index.ts", "export const a = () => 1;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: other"]);
}

#[test]
fn every_condition_of_a_subpath_import_is_reachable() {
    let diagnostics = lint(&[
        (
            "package.json",
            r##"{"name":"app","private":true,"main":"src/index.ts",
                "imports":{"#id":{"src":{"browser":"./src/id-browser.ts","default":"./src/id.ts"},"default":"./lib/id.js"}}}"##,
        ),
        ("src/index.ts", "import { id } from '#id';\nid();\n"),
        ("src/id.ts", "export const id = () => 1;\n"),
        ("src/id-browser.ts", "export const id = () => 2;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn a_subpath_import_pattern_is_not_an_entry() {
    let diagnostics = lint(&[
        (
            "package.json",
            r##"{"name":"app","private":true,"main":"src/index.ts","imports":{"#lib/*":"./src/lib/*.ts"}}"##,
        ),
        ("src/index.ts", "import { a } from '#lib/a';\na();\n"),
        ("src/lib/a.ts", "export const a = () => 1;\n"),
        ("src/lib/dead.ts", "export const dead = () => 2;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/lib/dead.ts: src/lib/dead.ts"]
    );
}

#[test]
fn augmenting_a_listed_package_uses_it() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","dependencies":{"dep":"1.0.0","other":"1.0.0"}}"#,
        ),
        (
            "src/index.ts",
            "declare module 'dep' { interface Options { a: string } }\nexport {};\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: other"]);
}

#[test]
fn augmenting_an_unlisted_package_is_not_an_unlisted_import() {
    let diagnostics = lint(&[
        ("package.json", r#"{"name":"app","private":true,"main":"src/index.ts"}"#),
        (
            "src/index.ts",
            "declare module 'dep' { interface Options { a: string } }\nexport {};\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_to_a_source_file_is_that_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "package.json",
        r#"{"name":"r","private":true,"workspaces":["packages/*"]}"#,
    );
    write("shared.ts", "export const used = 1;\nexport const dead = 2;\n");
    for name in ["a", "b"] {
        write(
            &format!("packages/{name}/package.json"),
            &format!(r#"{{"name":"{name}","private":true,"main":"index.ts"}}"#),
        );
        write(
            &format!("packages/{name}/index.ts"),
            "import { used } from './linked.ts';\nused;\n",
        );
        std::os::unix::fs::symlink("../../shared.ts", root.join(format!("packages/{name}/linked.ts"))).unwrap();
    }
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover(&root, &config).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(OsFs);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unused-exports"), ["shared.ts: dead"]);
}

#[test]
fn a_computed_relative_specifier_reaches_the_files_it_can_name() {
    let diagnostics = lint(&[
        ("package.json", r#"{"name":"app","private":true,"main":"lib/index.js"}"#),
        (
            "lib/index.js",
            "module.exports = (name) => [require(`./commands/${name}`), import(`./pages/${name}.js`)];\n",
        ),
        (
            "lib/commands/login.js",
            "const { open } = require('../utils.js');\nmodule.exports = () => open();\n",
        ),
        ("lib/utils.js", "exports.open = () => 1;\nexports.dead = () => 2;\n"),
        ("lib/pages/home.js", "export const page = 1;\n"),
        ("lib/pages/home.css.ts", "export const style = 1;\n"),
        ("lib/other/stray.js", "module.exports = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "lib/other/stray.js: lib/other/stray.js",
            "lib/pages/home.css.ts: lib/pages/home.css.ts"
        ]
    );
    assert_eq!(found(&diagnostics, "unused-exports"), ["lib/utils.js: dead"]);
}

#[test]
fn a_style_block_of_a_component_resolves_sass_partials() {
    let diagnostics = lint(&[
        ("package.json", r#"{"name":"app","private":true,"main":"src/App.vue"}"#),
        (
            "src/App.vue",
            "<template><div /></template>\n<style lang=\"scss\">\n@use '../css/mixins/focus';\n@use '../css/mixins/missing';\n</style>\n",
        ),
        ("css/mixins/_focus.scss", "@mixin ring {}\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        ["src/App.vue: ../css/mixins/missing"]
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn a_marker_only_package_json_does_not_make_a_project_of_files_outside_the_sources() {
    // `tools/gen` is ignored by git, so its file is no candidate; `{"type":"module"}` still makes no project of it.
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"scripts":{"gen":"node tools/gen/run.ts"}}"#,
        ),
        (".gitignore", "tools/gen/\n"),
        ("tools/gen/package.json", r#"{"type":"module"}"#),
        (
            "tools/gen/run.ts",
            "import Vinyl from \"vinyl\";\nconsole.log(new Vinyl());\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["tools/gen/run.ts: vinyl"]
    );
}
