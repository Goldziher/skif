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
    let memory = memory(files);
    let config = Config::from_toml_str("").unwrap();
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
        (
            "package.json",
            r#"{"name": "app", "version": "1.0.0", "private": true, "packageManager": "npm@11.0.0", "main": "src/index.js"}"#,
        ),
        (
            "src/index.js",
            "const p = require.resolve('./lib');\nconsole.log(p);\n",
        ),
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
    let text = explain(&ESM_FILES, "src/other.js", Some("b"));
    assert!(text.contains("    src/index.js:4 (resolve: its path, no export)\n"), "{text}");
    assert!(text.contains("  export `b` of src/other.js:\n"), "{text}");
    assert!(text.contains("    no module imports it by name or as a whole\n"), "{text}");
    assert!(text.contains("  => not used by another module\n"), "{text}");
}
