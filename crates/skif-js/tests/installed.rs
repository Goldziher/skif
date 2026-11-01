//! Binaries and peer dependencies read from installed packages in `node_modules`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::JsProvider;
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files
        .iter()
        .fold(MemoryFs::new(), |fs, (path, text)| fs.with_file(Path::new("/ws").join(path), text));
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let registry = Registry::new(vec![Box::new(JsProvider::new(fs))]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

fn names(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display(),
                d.dependency.clone().unwrap_or_default()
            )
        })
        .collect()
}

#[test]
fn installed_manifests_provide_binaries_and_peers() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"r","private":true,"workspaces":["packages/*"],
                "scripts":{"x":"acme-run build"},
                "devDependencies":{"@acme/toolkit":"1","react-dom":"1"}}"#,
        ),
        ("node_modules/@acme/toolkit/package.json", r#"{"name":"@acme/toolkit","bin":{"acme-run":"cli.js"}}"#),
        ("node_modules/next/package.json", r#"{"name":"next","peerDependencies":{"react-dom":"*"}}"#),
        (
            "packages/a/package.json",
            r#"{"name":"a","private":true,"dependencies":{"next":"1","react-dom":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "import next from \"next\";\nexport default next;\n"),
    ]);
    assert!(names(&diagnostics, "unlisted-binaries").is_empty(), "{diagnostics:#?}");
    assert!(names(&diagnostics, "unused-dependencies").is_empty(), "{diagnostics:#?}");
    assert_eq!(names(&diagnostics, "unused-dev-dependencies"), ["package.json: react-dom"]);
}

#[test]
fn optional_peer_dependencies_must_be_installed_for_development() {
    let diagnostics = lint(&[
        ("package.json", r#"{"name":"r","private":true,"workspaces":["packages/*"]}"#),
        (
            "packages/a/package.json",
            r#"{"name":"a","main":"src/index.ts","peerDependencies":{"sass":"1","less":"1"},
                "peerDependenciesMeta":{"sass":{"optional":true},"less":{"optional":true}},
                "devDependencies":{"less":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "import \"sass\";\nimport \"less\";\n"),
    ]);
    assert_eq!(
        names(&diagnostics, "unused-optional-peer-dependencies"),
        ["packages/a/package.json: sass"]
    );
}
