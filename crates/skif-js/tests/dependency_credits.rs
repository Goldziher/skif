//! Uses of listed dependencies that only a reference outside an ordinary import makes: a re-export inside a
//! `declare module` block, a package file a stylesheet `url()` names and a file of a subpath directory with a
//! `package.json` of its own, and the `@types` package that a package shipping its own types does not use.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (a.rule.as_str(), &a.location.path, &a.message).cmp(&(b.rule.as_str(), &b.location.path, &b.message))
    });
    diagnostics
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display(),
                d.dependency.clone().unwrap_or_else(|| d.message.clone())
            )
        })
        .collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

#[test]
fn reexports_inside_declare_module_blocks_use_the_packages_they_name() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"@commander-js/extra-typings":"12.0.0","@types/react-is":"19.0.0","left-pad":"1.3.0"}}"#,
        ),
        ("src/index.ts", "export const main = (): number => 1;\n"),
        (
            "src/commander.d.ts",
            "declare module 'commander' {\n  export * from '@commander-js/extra-typings';\n}\n",
        ),
        (
            "src/react-is.d.ts",
            "declare module 'compiled/react-is' {\n  export { isElement } from 'react-is';\n}\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["package.json: left-pad"]
    );
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
}
