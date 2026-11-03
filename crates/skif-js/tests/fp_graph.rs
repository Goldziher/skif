//! False-positive classes of the module graph: signature references, promise callbacks, comments, ambient modules,
//! duplicate exports, entries outside the declaring package, discovery and fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
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
