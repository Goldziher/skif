//! Files whose imports skif extracts without a compiler: MDX documents, stylesheets of other languages and framework
//! conventions. Each fixture is linted with the plugin table, the way the CLI does.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

fn load(dir: &Path, base: &Path, memory: &mut MemoryFs, config: &mut Option<String>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            load(&path, base, memory, config);
            continue;
        }
        let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(&path).unwrap();
        if rel == "skif.toml" {
            *config = Some(text);
        } else {
            memory.insert(Path::new(ROOT).join(&rel), text);
        }
    }
}

fn lint_fixture(case: &str) -> Vec<Diagnostic> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(case);
    let mut memory = MemoryFs::new();
    let mut toml = None;
    load(&base, &base, &mut memory, &mut toml);
    let config = Config::from_toml_str(&toml.unwrap_or_default()).unwrap();
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

/// `path: subject` of every diagnostic of `rule`; the subject is the dependency or the first quoted name.
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
fn mdx_docs_snapshot() {
    insta::assert_json_snapshot!("mdx-docs", lint_fixture("mdx-docs"));
}

#[test]
fn mdx_imports_keep_components_and_dependencies_used() {
    let diagnostics = lint_fixture("mdx-docs");
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/components/Orphan.jsx: src/components/Orphan.jsx"]
    );
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["package.json: unused-dependency"]
    );
    assert_eq!(
        found(&diagnostics, "unused-namespace-exports"),
        ["src/components/ui.js: Unread"]
    );
    // Code samples, inline code and frontmatter are not imports.
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), Vec::<String>::new());
}
