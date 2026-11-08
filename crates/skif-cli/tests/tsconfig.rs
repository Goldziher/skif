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

/// The JSON document of a run over `path`.
fn lint(path: &std::path::Path, args: &[&str]) -> (support::Run, Value) {
    let result = run(skif().args(["--format", "json"]).args(args).arg(path));
    let report = result.json();
    (result, report)
}

#[test]
fn a_tsconfig_that_does_not_exist_is_a_configuration_error() {
    let dir = fixture("tsconfig");
    let (result, error) = lint(dir.path(), &["--tsconfig", "tsconfig.missing.json"]);
    assert_eq!(result.code, 2);
    assert_eq!(error["error"]["kind"], "config", "{error:#}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("`tsconfig.missing.json` does not exist"),
        "{error:#}"
    );

    std::fs::write(
        dir.path().join("skif.toml"),
        "[js]\ntsconfig = \"tsconfig.missing.json\"\n",
    )
    .unwrap();
    let (result, error) = lint(dir.path(), &[]);
    assert_eq!(result.code, 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("[js] tsconfig: `tsconfig.missing.json`"),
        "{error:#}"
    );
}

#[test]
fn a_tsconfig_outside_the_workspace_root_is_rejected() {
    for path in ["../tsconfig.json", "/etc/tsconfig.json"] {
        let dir = fixture("tsconfig");
        let (result, error) = lint(dir.path(), &["--tsconfig", path]);
        assert_eq!(result.code, 2, "{path}");
        assert_eq!(error["error"]["kind"], "config", "{path}: {error:#}");
        assert!(
            error["error"]["message"].as_str().unwrap().contains("js.tsconfig"),
            "{path}: {error:#}"
        );
    }
}

#[test]
fn tsconfig_applies_to_the_roots_that_have_it() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["with", "without"] {
        let root = dir.path().join(name);
        std::fs::create_dir(&root).unwrap();
        support::copy_fixture("tsconfig", &root);
    }
    std::fs::remove_file(dir.path().join("without/tsconfig.app.json")).unwrap();
    let (result, report) = lint(dir.path(), &["-r", "--tsconfig", "tsconfig.app.json"]);
    assert_ne!(result.code, 2, "{report:#}{}", result.stderr);
    let unresolved: Vec<&str> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["rule"] == "unresolved-imports")
        .map(|diagnostic| diagnostic["location"]["path"].as_str().unwrap())
        .collect();
    // Only the root without the file keeps its default configuration, where `~/greet` does not resolve.
    assert_eq!(unresolved, ["without/src/index.ts"], "{report:#}");

    let (result, error) = lint(dir.path(), &["-r", "--tsconfig", "tsconfig.nowhere.json"]);
    assert_eq!(result.code, 2);
    assert_eq!(error["error"]["kind"], "config", "{error:#}");
}

#[test]
fn use_tsconfig_files_warns_about_an_empty_project() {
    let dir = fixture("tsconfig");
    std::fs::write(dir.path().join("packages/a/tsconfig.json"), "{ \"files\": [] }\n").unwrap();
    let result = run(skif()
        .args(["--format", "json", "--use-tsconfig-files"])
        .arg(dir.path()));
    assert!(
        result
            .stderr
            .contains("packages/a/tsconfig.json: selects no file of its package"),
        "{}",
        result.stderr
    );
    // stdout keeps exactly the report.
    let _ = result.json();
    let quiet = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert!(!quiet.stderr.contains("selects no file"), "{}", quiet.stderr);
}

#[test]
fn a_root_without_javascript_analysis_needs_no_tsconfig() {
    let dir = fixture("tsconfig");
    let (result, report) = lint(dir.path(), &["--no-js", "--tsconfig", "tsconfig.missing.json"]);
    assert_ne!(result.code, 2, "{report:#}{}", result.stderr);

    std::fs::write(
        dir.path().join("skif.toml"),
        "[js]\nenabled = false\ntsconfig = \"tsconfig.missing.json\"\n",
    )
    .unwrap();
    let (result, report) = lint(dir.path(), &[]);
    assert_ne!(result.code, 2, "{report:#}{}", result.stderr);
}

#[test]
fn tsconfig_is_looked_for_only_in_roots_with_javascript_analysis() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["js-off", "js-on"] {
        let root = dir.path().join(name);
        std::fs::create_dir(&root).unwrap();
        support::copy_fixture("tsconfig", &root);
    }
    std::fs::write(dir.path().join("js-off/skif.toml"), "[js]\nenabled = false\n").unwrap();
    std::fs::remove_file(dir.path().join("js-on/tsconfig.app.json")).unwrap();
    // Only the root without JavaScript analysis has the file, so the flag names nothing the run reads.
    let (result, error) = lint(dir.path(), &["-r", "--tsconfig", "tsconfig.app.json"]);
    assert_eq!(result.code, 2, "{error:#}");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains("does not exist in"), "{error:#}");
    assert!(message.contains("js-on") && !message.contains("js-off"), "{error:#}");
}

#[test]
fn tsconfig_exists_as_the_analysis_reads_it() {
    let dir = fixture("tsconfig");
    let (result, error) = lint(dir.path(), &["--tsconfig", "./src/../tsconfig.missing.json"]);
    assert_eq!(result.code, 2, "{error:#}");
    assert_eq!(error["error"]["kind"], "config", "{error:#}");

    std::fs::create_dir(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/tsconfig.json"),
        "{ \"compilerOptions\": { \"baseUrl\": \"..\", \"paths\": { \"~/*\": [\"./src/*\"] } }, \"include\": [\"../src\"] }\n",
    )
    .unwrap();
    let (result, report) = lint(dir.path(), &["--tsconfig", "config\\tsconfig.json"]);
    assert_ne!(result.code, 2, "{report:#}{}", result.stderr);
    let unresolved = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["rule"] == "unresolved-imports")
        .count();
    assert_eq!(unresolved, 0, "{report:#}");
}

#[test]
fn trace_dependency_rejects_a_tsconfig_outside_the_workspace_root() {
    for path in ["../tsconfig.json", "/etc/tsconfig.json"] {
        let dir = fixture("tsconfig");
        let (result, error) = lint(dir.path(), &["--trace-dependency", "left-pad", "--tsconfig", path]);
        assert_eq!(result.code, 2, "{path}: {error:#}");
        assert_eq!(error["error"]["kind"], "config", "{path}: {error:#}");
        assert!(
            error["error"]["message"].as_str().unwrap().contains("js.tsconfig"),
            "{path}: {error:#}"
        );
    }
}
