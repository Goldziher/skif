//! A plugin config whose entries are computed (an rspack config exporting a function) leaves a package incomplete,
//! unless the package names its entries itself: an explicit `entry` that applies in the run mode completes it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, IncompleteCause, IncompletePackage, MemoryFs};
use skif_js::{InputSource, JsAnalysis, JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const FILES: [(&str, &str); 5] = [
    (
        "package.json",
        r#"{"name":"web","version":"1.0.0","private":true,"scripts":{"build":"rspack build"},"devDependencies":{"@rspack/cli":"1.0.0"}}"#,
    ),
    (
        "rspack.config.js",
        "const fs = require('fs');\n\
         module.exports = (env) => ({\n\
           entry: Object.fromEntries(fs.readdirSync('src').map((f) => [f, `./src/${f}`])),\n\
         });\n",
    ),
    ("src/main.js", "import { used } from './lib.js';\nconsole.log(used);\n"),
    ("src/lib.js", "export const used = 1;\nexport const unusedExport = 2;\n"),
    ("src/orphan.js", "export const orphan = 3;\n"),
];

fn run(toml: &str) -> (Vec<IncompletePackage>, Vec<Diagnostic>) {
    let memory = FILES.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = || {
        PluginSource::new(config.js.plugins.clone())
            .with_workspaces(&config.js.workspaces)
            .with_overrides(&config.overrides)
    };
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(source())];
    let incomplete = JsAnalysis::build(&workspace, &config, &fs, &sources, true).incomplete_packages();
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source()));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    (incomplete, diagnostics)
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| d.location.path.display().to_string())
        .collect();
    found.sort();
    found
}

#[test]
fn an_explicit_production_entry_completes_a_package_with_computed_entries() {
    let (incomplete, diagnostics) = run("[js]\nproduction = true\nentry = [\"src/main.js!\"]\n");
    assert_eq!(incomplete, Vec::new());
    assert_eq!(found(&diagnostics, "unused-files"), ["src/orphan.js"]);
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/lib.js"]);
}

#[test]
fn a_plain_entry_does_not_complete_a_production_run() {
    let (incomplete, _) = run("[js]\nproduction = true\nentry = [\"src/main.js\"]\n");
    assert_eq!(incomplete.len(), 1);
}

#[test]
fn without_an_entry_the_package_stays_incomplete() {
    let (incomplete, diagnostics) = run("[js]\nproduction = true\n");
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].reasons[0].cause, IncompleteCause::ComputedEntries);
    assert_eq!(found(&diagnostics, "unused-files"), Vec::<String>::new());
}

#[test]
fn a_plain_entry_completes_a_normal_run() {
    let (incomplete, _) = run("[js]\nentry = [\"src/main.js\"]\n");
    assert_eq!(incomplete, Vec::new());
}
