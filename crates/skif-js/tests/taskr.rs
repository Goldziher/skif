//! The `taskr` plugin (ADR 0020), linted from the `taskr` fixture with the plugin table.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

fn load(dir: &Path, base: &Path, memory: &mut MemoryFs) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            load(&path, base, memory);
            continue;
        }
        let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
        memory.insert(Path::new(ROOT).join(rel), fs::read_to_string(&path).unwrap());
    }
}

fn lint(toml: &str) -> Vec<Diagnostic> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/taskr");
    let mut memory = MemoryFs::new();
    load(&base, &base, &mut memory);
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (
            a.rule.as_str(),
            &a.location.path,
            a.location.span.map(|s| s.start),
            &a.message,
        )
            .cmp(&(
                b.rule.as_str(),
                &b.location.path,
                b.location.span.map(|s| s.start),
                &b.message,
            ))
    });
    diagnostics
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
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
        .collect()
}

#[test]
fn taskr_snapshot() {
    let diagnostics = lint("");
    insta::assert_json_snapshot!("taskr", diagnostics);
}

#[test]
fn taskr_credits_the_dependencies_of_its_taskfiles_and_plugins() {
    let diagnostics = lint("");
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["app/package.json: left-over"],
        "{diagnostics:#?}"
    );
    let disabled = lint("[js.plugins]\ntaskr = false\n");
    let unused = found(&disabled, "unused-dev-dependencies");
    for name in ["@taskr/clear", "@vercel/ncc", "node-notifier"] {
        assert!(
            unused.contains(&format!("app/package.json: {name}")),
            "{name}: {unused:#?}"
        );
    }
}
