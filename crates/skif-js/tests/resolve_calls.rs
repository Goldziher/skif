//! `require.resolve` and `import.meta.resolve`: the call returns a module's path and loads nothing, so it keeps a local
//! file in use and uses the package it names, but uses no export of the module.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{InputSource, JsAnalysis, JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const ESM_PACKAGE: (&str, &str) = (
    "package.json",
    r#"{"name": "app", "version": "1.0.0", "private": true, "packageManager": "npm@11.0.0", "type": "module", "main": "src/index.js"}"#,
);

const ESM_PACKAGE_TS: (&str, &str) = (
    "package.json",
    r#"{"name": "app", "version": "1.0.0", "private": true, "packageManager": "npm@11.0.0", "type": "module", "main": "src/index.ts"}"#,
);

const ESM_FILES: [(&str, &str); 4] = [
    ESM_PACKAGE,
    (
        "src/index.js",
        "import { createRequire } from 'node:module';\n\
         const require = createRequire(import.meta.url);\n\
         const p = require.resolve('./lib.cjs');\n\
         const q = import.meta.resolve('./other.js');\n\
         console.log(p, q);\n",
    ),
    ("src/lib.cjs", "exports.a = 1;\n"),
    ("src/other.js", "export const b = 2;\n"),
];

fn memory(files: &[(&str, &str)]) -> MemoryFs {
    files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    })
}

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    lint_with(files, "")
}

fn lint_with(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = memory(files);
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `<path>: <name>` of every diagnostic of `rule`, sorted: the dependency, else the first backticked part of the
/// message.
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

fn explain(files: &[(&str, &str)], file: &str, export: Option<&str>) -> String {
    let memory = memory(files);
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = Vec::new();
    JsAnalysis::build(&workspace, &config, &fs, &sources, true)
        .explain_usage(Path::new(file), export)
        .render()
}

const CJS_PACKAGE: (&str, &str) = (
    "package.json",
    r#"{"name": "app", "version": "1.0.0", "private": true, "packageManager": "npm@11.0.0", "main": "src/index.js"}"#,
);

#[test]
fn esm_resolve_calls_keep_the_files_used_and_use_no_export() {
    let diagnostics = lint(&ESM_FILES);
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        ["src/lib.cjs: a", "src/other.js: b"]
    );
}

#[test]
fn a_commonjs_resolve_call_keeps_the_file_used_and_uses_no_export() {
    let diagnostics = lint(&[
        CJS_PACKAGE,
        ("src/index.js", "const p = require.resolve('./lib');\nconsole.log(p);\n"),
        ("src/lib.js", "exports.a = 1;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/lib.js: a"]);
}

#[test]
fn a_resolved_package_is_a_used_dependency() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name": "app", "version": "1.0.0", "private": true, "packageManager": "npm@11.0.0", "type": "module", "main": "src/index.js", "dependencies": {"cjs-kit": "1.0.0", "esm-kit": "1.0.0", "idle-kit": "1.0.0"}}"#,
        ),
        (
            "src/index.js",
            "import { createRequire } from 'node:module';\n\
             const require = createRequire(import.meta.url);\n\
             console.log(require.resolve('cjs-kit'), import.meta.resolve('esm-kit'));\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: idle-kit"]);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
}

#[test]
fn explain_usage_names_a_resolve_call_as_taking_no_export() {
    assert_eq!(
        explain(&ESM_FILES, "src/other.js", Some("b")),
        concat!(
            "src/other.js\n",
            "  imported by 1 place(s):\n",
            "    src/index.js:4 (resolve: its path, no export)\n",
            "  reached from an entry through: src/index.js -> src/other.js\n",
            "  export `b` of src/other.js:\n",
            "    no module imports it by name or as a whole\n",
            "  => not used by another module\n",
        )
    );
}

/// A resolve call uses no export, but types were never credited by one: a type export of a module only resolved is
/// reported by `unused-types`, as its values are by `unused-exports`.
#[test]
fn a_type_export_of_a_resolved_module_is_reported() {
    let diagnostics = lint(&[
        ESM_PACKAGE_TS,
        (
            "src/index.ts",
            "import { createRequire } from 'node:module';\n\
             const require = createRequire(import.meta.url);\n\
             console.log(require.resolve('./lib.ts'));\n",
        ),
        (
            "src/lib.ts",
            "export type Shape = { a: number };\nexport const a = 1;\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unused-types"), ["src/lib.ts: Shape"]);
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/lib.ts: a"]);
}

/// A resolved module is a module of the graph: what it imports is followed.
#[test]
fn what_a_resolved_module_requires_is_used() {
    let diagnostics = lint(&[
        CJS_PACKAGE,
        ("src/index.js", "console.log(require.resolve('./lib'));\n"),
        ("src/lib.js", "require('./dep');\nexports.a = 1;\n"),
        ("src/dep.js", "console.log(1);\n"),
        ("src/stray.js", "console.log(2);\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), ["src/stray.js: src/stray.js"]);
}

/// A resolve call of a missing file or an unlisted package is reported as an import of it would be.
#[test]
fn an_unresolvable_resolve_call_is_reported() {
    let diagnostics = lint(&[
        CJS_PACKAGE,
        (
            "src/index.js",
            "console.log(require.resolve('./missing'), require.resolve('nope'));\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unresolved-imports"), ["src/index.js: ./missing"]);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), ["src/index.js: nope"]);
}

/// A resolve call does not change how a module read through a namespace is judged: it stays namespace-only, so its
/// unread exports are `unused-namespace-exports`, not `unused-exports`.
#[test]
fn a_resolve_call_leaves_a_namespace_only_module_namespace_only() {
    let diagnostics = lint(&[
        ESM_PACKAGE,
        (
            "src/index.js",
            "import * as ns from './lib.js';\nconsole.log(ns.a, import.meta.resolve('./lib.js'));\n",
        ),
        ("src/lib.js", "export const a = 1;\nexport const b = 2;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-namespace-exports"), ["src/lib.js: b"]);
    assert_eq!(found(&diagnostics, "unused-exports"), none());
}

/// A resolve call loads nothing, so it closes no import cycle; a `require` of the same module does.
#[test]
fn a_resolve_call_closes_no_import_cycle() {
    let cases: [(&str, &[&str]); 2] = [
        ("module.exports = require.resolve('./a');\n", &[]),
        (
            "module.exports = require('./a');\n",
            &["src/a.js: 2 modules import each other in a cycle"],
        ),
    ];
    for (b, cycles) in cases {
        let diagnostics = lint_with(
            &[
                (
                    "package.json",
                    r#"{"name": "app", "version": "1.0.0", "private": true, "packageManager": "npm@11.0.0", "main": "src/a.js"}"#,
                ),
                ("src/a.js", "const b = require('./b');\nconsole.log(b);\n"),
                ("src/b.js", b),
            ],
            "[rules]\ncircular-imports = \"warn\"\n",
        );
        assert_eq!(found(&diagnostics, "circular-imports"), cycles, "{b}");
    }
}

/// A module that an extensionless mise file task only resolves is used, but no entry, so its exports are judged.
#[test]
fn a_module_a_mise_file_task_resolves_is_used_and_its_exports_are_judged() {
    let diagnostics = lint(&[
        CJS_PACKAGE,
        ("mise.toml", "[tools]\nnode = \"22\"\n"),
        (
            "mise-tasks/gen",
            "#!/usr/bin/env node\nconsole.log(require.resolve('../src/data.js'));\n",
        ),
        ("src/index.js", "console.log(1);\n"),
        ("src/data.js", "exports.a = 1;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/data.js: a"]);
}

/// A computed relative resolve call is followed as a glob, as a computed `require` is: the files it can name are used,
/// and, as for the glob of a `require`, read in full.
#[test]
fn a_computed_relative_resolve_call_reaches_the_files_it_can_name() {
    let diagnostics = lint(&[
        CJS_PACKAGE,
        (
            "src/index.js",
            "module.exports = (name) => require.resolve(`./tasks/${name}`);\n",
        ),
        ("src/tasks/build.js", "exports.run = () => 1;\n"),
        ("src/other/stray.js", "module.exports = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/other/stray.js: src/other/stray.js"]
    );
    assert_eq!(found(&diagnostics, "unused-exports"), none());
}
