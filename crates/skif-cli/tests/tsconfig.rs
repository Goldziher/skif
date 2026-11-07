//! `--tsconfig` and `--use-tsconfig-files`: the root package's TypeScript configuration and project files taken from
//! the `tsconfig` of each package.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;

use serde_json::Value;
use support::{fixture, run, skif};

/// `rule path` of every diagnostic of the JSON report of a run with `args`.
fn findings(args: &[&str]) -> BTreeSet<String> {
    let dir = fixture("tsconfig");
    let result = run(skif().args(["--format", "json"]).args(args).arg(dir.path()));
    let report: Value = result.json();
    report["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("{report:#}"))
        .iter()
        .map(|diagnostic| {
            format!(
                "{} {}",
                diagnostic["rule"].as_str().unwrap(),
                diagnostic["location"]["path"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn tsconfig_names_the_root_packages_typescript_configuration() {
    let default = findings(&[]);
    assert!(
        default.iter().any(|finding| finding.starts_with("unresolved-imports ")),
        "{default:#?}"
    );
    let configured = findings(&["--tsconfig", "tsconfig.app.json"]);
    assert!(
        !configured
            .iter()
            .any(|finding| finding.starts_with("unresolved-imports ")),
        "{configured:#?}"
    );
    assert!(configured.contains("unused-files scripts/tool.ts"), "{configured:#?}");
    assert_eq!(findings(&["--tsConfig", "tsconfig.app.json"]), configured);
    assert_eq!(findings(&["-t", "tsconfig.app.json"]), configured);
}

#[test]
fn use_tsconfig_files_limits_the_project_to_the_tsconfig_files() {
    let unused: Vec<String> = findings(&["--tsConfig", "tsconfig.app.json", "--use-tsconfig-files"])
        .into_iter()
        .filter(|finding| finding.starts_with("unused-files "))
        .collect();
    assert_eq!(
        unused,
        ["unused-files packages/a/lib/unused.ts", "unused-files src/orphan.ts"]
    );
}
