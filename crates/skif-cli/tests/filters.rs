//! knip's run-time filters on the lint command: `--workspace` and the flags that narrow or bound what is reported.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;

use serde_json::Value;
use support::{fixture, redact, run, skif};

/// `(rule, path)` of every diagnostic of a JSON report.
fn findings(report: &Value) -> BTreeSet<(String, String)> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| {
            (
                diagnostic["rule"].as_str().unwrap().to_owned(),
                diagnostic["location"]["path"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn lint_json(args: &[&str]) -> (support::Run, Value) {
    let dir = fixture("filters");
    let result = run(skif().args(["--format", "json"]).args(args).arg(dir.path()));
    let report = result.json();
    (result, report)
}

#[test]
fn workspace_reports_only_the_selected_packages() {
    let dir = fixture("filters");
    let by_dir = run(skif()
        .args(["--color", "never", "--workspace", "./packages/a/"])
        .arg(dir.path()));
    assert_eq!(by_dir.code, 1, "{}{}", by_dir.stdout, by_dir.stderr);
    insta::assert_snapshot!("workspace_a_human", redact(&by_dir.stdout, dir.path()));

    let (_, by_name) = lint_json(&["-W", "@cli/a"]);
    let (_, by_dir) = lint_json(&["-W", "packages/a"]);
    assert_eq!(findings(&by_name), findings(&by_dir));
    assert!(
        findings(&by_name)
            .iter()
            .all(|(_, path)| path.starts_with("packages/a/")),
        "{by_name:#}"
    );
    assert_eq!(by_name["summary"]["packages"], 1);
}

#[test]
fn workspace_accepts_globs_and_exclusions() {
    let (_, all) = lint_json(&[]);
    let (_, globbed) = lint_json(&["--workspace", "packages/*"]);
    assert_eq!(findings(&all), findings(&globbed));
    let (_, not_a) = lint_json(&["--workspace", "!@cli/a"]);
    assert!(
        findings(&not_a)
            .iter()
            .all(|(_, path)| !path.starts_with("packages/a/")),
        "{not_a:#}"
    );
    assert!(!findings(&not_a).is_empty());
}

#[test]
fn workspace_that_matches_nothing_is_a_usage_error() {
    let (result, error) = lint_json(&["--workspace", "@cli/missing"]);
    assert_eq!(result.code, 2);
    assert_eq!(error["error"]["kind"], "usage", "{error:#}");
}

#[test]
fn workspace_limits_what_fix_changes() {
    let dir = fixture("filters");
    let result = run(skif()
        .args([
            "--fix-unsafe",
            "--no-install",
            "--no-prompt",
            "--workspace",
            "@cli/b",
            "--format",
            "json",
        ])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(dir.path().join("packages/a/orphan.ts").exists());
    let a = std::fs::read_to_string(dir.path().join("packages/a/package.json")).unwrap();
    assert!(a.contains("is-number"), "{a}");
    let b = std::fs::read_to_string(dir.path().join("packages/b/package.json")).unwrap();
    assert!(!b.contains("is-even"), "{b}");
}

#[test]
fn strict_uses_only_each_packages_own_dependencies() {
    let unlisted = ("unlisted-dependencies".to_owned(), "packages/a/index.ts".to_owned());
    let (_, production) = lint_json(&["--production"]);
    assert!(!findings(&production).contains(&unlisted), "{production:#}");
    let (result, strict) = lint_json(&["--strict"]);
    assert_eq!(result.code, 1);
    // `left-pad` is listed by the root only, which strict mode does not consult for `@cli/a`.
    assert!(findings(&strict).contains(&unlisted), "{strict:#}");
    assert!(findings(&strict).is_superset(&findings(&production)));
}
