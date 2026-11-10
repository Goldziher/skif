//! The toolchain rules over an on-disk workspace: a Dockerfile, a workflow, version files and a shell script, plus a
//! project outside the workspace that is not read.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{OsFs, Report};
use skif_lint::{Registry, RunOptions, run};
use skif_workspace::Workspace;

fn report() -> Report {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/toolchain");
    let config = Config::default();
    let workspace = Workspace::discover(&root, &config).unwrap();
    let registry = Registry::new(vec![Box::new(skif_rules::toolchain_provider(Arc::new(OsFs)))]).unwrap();
    run(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

#[test]
fn toolchain_fixture_findings() {
    let report = report();
    let mut found: Vec<(String, String, u32)> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.rule.as_str().to_owned(),
                diagnostic.location.path.to_string_lossy().replace('\\', "/"),
                diagnostic.location.span.unwrap().start_line,
            )
        })
        .collect();
    found.sort();
    let expected = [
        ("foreign-package-manager-command", ".github/workflows/ci.yml", 16),
        ("foreign-package-manager-command", "Dockerfile", 5),
        ("foreign-package-manager-command", "Dockerfile", 6),
        ("foreign-package-manager-command", "packages/web/scripts/deploy.sh", 3),
        ("toolchain-pin-drift", ".github/workflows/ci.yml", 10),
        ("toolchain-pin-drift", ".github/workflows/ci.yml", 13),
        ("toolchain-pin-drift", "Dockerfile", 1),
        ("toolchain-pin-drift", "Dockerfile", 2),
        ("toolchain-pin-drift", "packages/web/.node-version", 1),
    ]
    .map(|(rule, path, line)| (rule.to_owned(), path.to_owned(), line));
    assert_eq!(found, expected);
    insta::assert_snapshot!(serde_json::to_string_pretty(&report).unwrap());
}
