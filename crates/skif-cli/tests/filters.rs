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

fn rules(report: &Value) -> BTreeSet<String> {
    findings(report).into_iter().map(|(rule, _)| rule).collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn shortcuts_report_one_group_of_knip_issue_types() {
    let (_, files) = lint_json(&["--files"]);
    assert_eq!(rules(&files), set(&["unused-files"]));
    let (_, dependencies) = lint_json(&["--dependencies"]);
    assert_eq!(rules(&dependencies), set(&["unused-dependencies"]));
    let (_, exports) = lint_json(&["--exports"]);
    assert_eq!(rules(&exports), set(&["unused-exports"]));
    // `circular-imports` is off by default; including it turns it on, unless a flag turned it off.
    let (result, cycles) = lint_json(&["--cycles"]);
    assert_eq!(result.code, 1);
    assert_eq!(rules(&cycles), set(&["circular-imports"]));
    let (result, off) = lint_json(&["--cycles", "--ignore-rule", "circular-imports"]);
    assert_eq!(result.code, 0);
    assert_eq!(rules(&off), set(&[]));
}

#[test]
fn include_and_exclude_take_issue_types_and_rule_ids() {
    let (_, included) = lint_json(&["--include", "files,unused-exports"]);
    assert_eq!(rules(&included), set(&["unused-exports", "unused-files"]));
    let (_, excluded) = lint_json(&["--exclude", "dependencies", "--exclude", "unused-files"]);
    assert_eq!(rules(&excluded), set(&["unused-exports"]));
    let (result, error) = lint_json(&["--include", "nonsense"]);
    assert_eq!(result.code, 2);
    assert_eq!(error["error"]["kind"], "usage", "{error:#}");
}

#[test]
fn tags_leave_out_tagged_exports() {
    let (_, exports) = lint_json(&["--exports"]);
    assert_eq!(rules(&exports), set(&["unused-exports"]));
    let (result, tagged) = lint_json(&["--exports", "--tags=-custom"]);
    assert_eq!(result.code, 0);
    assert_eq!(rules(&tagged), set(&[]));
    let (result, error) = lint_json(&["--tags", "+custom"]);
    assert_eq!(result.code, 2);
    assert_eq!(error["error"]["kind"], "usage", "{error:#}");
}

#[test]
fn max_issues_and_no_exit_code_change_only_the_exit_code() {
    // The fixture has two errors.
    let (result, report) = lint_json(&["--max-issues", "2"]);
    assert_eq!(result.code, 0);
    assert_eq!(report["summary"]["errors"], 2);
    let (result, _) = lint_json(&["--max-issues", "1"]);
    assert_eq!(result.code, 1);
    let (result, report) = lint_json(&["--no-exit-code"]);
    assert_eq!(result.code, 0);
    assert_eq!(report["summary"]["errors"], 2);
    let (result, _) = lint_json(&["--no-exit-code", "--workspace", "@cli/missing"]);
    assert_eq!(result.code, 2);
}

#[test]
fn max_show_issues_shortens_the_list_but_not_the_summary() {
    let dir = fixture("filters");
    let human = run(skif()
        .args(["--color", "never", "--max-show-issues", "1"])
        .arg(dir.path()));
    assert_eq!(human.code, 1);
    assert!(
        human.stderr.contains("left 1 finding out of the list"),
        "{}",
        human.stderr
    );
    insta::assert_snapshot!("max_show_issues_human", redact(&human.stdout, dir.path()));

    let (_, all) = lint_json(&[]);
    let (_, shown) = lint_json(&["--max-show-issues", "1"]);
    assert_eq!(shown["summary"], all["summary"]);
    assert_eq!(shown["diagnostics"].as_array().unwrap().len(), 3);
}

#[test]
fn include_entry_exports_reports_exports_of_entry_files() {
    let entry = ("unused-exports".to_owned(), "packages/a/index.ts".to_owned());
    let (_, default) = lint_json(&["--exports"]);
    assert!(!findings(&default).contains(&entry), "{default:#}");
    let (_, included) = lint_json(&["--exports", "--include-entry-exports"]);
    assert!(findings(&included).contains(&entry), "{included:#}");
}

#[test]
fn run_diagnostics_go_to_stderr() {
    let dir = fixture("filters");
    let plain = run(skif().args(["--format", "json"]).arg(dir.path()));
    let traced = run(skif()
        .args(["--format", "json", "--debug", "--duration", "--memory", "--performance"])
        .arg(dir.path()));
    // stdout keeps exactly the report.
    assert_eq!(traced.json(), plain.json());
    let debug: Vec<&str> = traced
        .stderr
        .lines()
        .filter(|line| line.starts_with("skif debug:"))
        .collect();
    insta::assert_snapshot!("debug_lines", redact(&debug.join("\n"), dir.path()));
    assert!(traced.stderr.contains("skif timing: lint roots: "), "{}", traced.stderr);
    assert!(traced.stderr.contains("skif: finished in "), "{}", traced.stderr);
    assert!(traced.stderr.contains("skif: peak memory"), "{}", traced.stderr);
    assert!(!plain.stderr.contains("skif"), "{}", plain.stderr);
}

#[test]
fn trace_dependency_lists_manifests_imports_and_users() {
    let dir = fixture("filters");
    let human = run(skif().args(["--trace-dependency", "left-pad"]).arg(dir.path()));
    assert_eq!(human.code, 0, "{}{}", human.stdout, human.stderr);
    insta::assert_snapshot!("trace_left_pad_human", redact(&human.stdout, dir.path()));

    let (result, unused) = lint_json(&["--trace-dependency", "is-number"]);
    assert_eq!(result.code, 0);
    let trace = &unused["traces"][0];
    assert_eq!(trace["root"], ".");
    assert_eq!(trace["listed-in"][0]["manifest"], "packages/a/package.json");
    assert_eq!(trace["imported-by"], Value::Array(Vec::new()));
    assert_eq!(trace["used-by"], Value::Array(Vec::new()));

    let (result, error) = lint_json(&["--trace-dependency", "left-pad", "--fix"]);
    assert_eq!(result.code, 2);
    assert_eq!(error["error"]["kind"], "usage", "{error:#}");
}

#[test]
fn no_gitignore_analyses_ignored_files() {
    let dir = fixture("filters");
    std::fs::write(dir.path().join(".gitignore"), "packages/a/orphan.ts\n").unwrap();
    let orphan = ("unused-files".to_owned(), "packages/a/orphan.ts".to_owned());
    let honoured = run(skif().args(["--format", "json"]).arg(dir.path())).json();
    assert!(!findings(&honoured).contains(&orphan), "{honoured:#}");
    let ignored = run(skif().args(["--format", "json", "--no-gitignore"]).arg(dir.path())).json();
    assert!(findings(&ignored).contains(&orphan), "{ignored:#}");
}

#[test]
fn findings_of_no_package_belong_to_the_root() {
    // `unmatched-workspace-patterns` reports `missing/*` in the root manifest, with no package, and fixes it there.
    let root = ("unmatched-workspace-patterns".to_owned(), "package.json".to_owned());
    for (workspace, selected) in [("@cli/b", false), (".", true)] {
        let dir = fixture("filters-root");
        let report = run(skif()
            .args(["--format", "json", "--workspace", workspace])
            .arg(dir.path()))
        .json();
        assert_eq!(
            findings(&report).contains(&root),
            selected,
            "-W {workspace}: {report:#}"
        );

        let fixed = run(skif()
            .args([
                "--fix-unsafe",
                "--no-install",
                "--no-prompt",
                "--format",
                "json",
                "--workspace",
                workspace,
            ])
            .arg(dir.path()));
        assert_eq!(fixed.code, 0, "{}{}", fixed.stdout, fixed.stderr);
        let manifest = std::fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert_eq!(!manifest.contains("missing/*"), selected, "-W {workspace}: {manifest}");
    }
}

#[test]
fn include_turns_on_a_rule_the_configuration_turned_off() {
    let dir = fixture("filters");
    std::fs::write(dir.path().join("skif.toml"), "[rules]\nunused-files = \"off\"\n").unwrap();
    let lint = |args: &[&str]| run(skif().args(["--format", "json"]).args(args).arg(dir.path())).json();
    assert!(!rules(&lint(&[])).contains("unused-files"));
    let included = lint(&["--include", "files"]);
    assert_eq!(rules(&included), set(&["unused-files"]));
    // Back at its default severity, not raised to `error`.
    assert_eq!(included["diagnostics"][0]["severity"], "warning", "{included:#}");
    assert_eq!(
        rules(&lint(&["--include", "files", "--rule", "unused-files=off"])),
        set(&[])
    );
}

#[test]
fn tags_need_ignore_tags_to_be_a_list() {
    let dir = fixture("filters");
    std::fs::write(
        dir.path().join("skif.toml"),
        "[rules]\nunused-exports = { level = \"warn\", ignore-tags = \"custom\" }\n",
    )
    .unwrap();
    let result = run(skif().args(["--format", "json", "--tags=-other"]).arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    let error = result.json();
    assert!(
        error["error"]["message"].as_str().unwrap().contains("ignore-tags"),
        "{error:#}"
    );
}

#[test]
fn max_show_issues_is_for_reading_only() {
    for args in [
        &["--format", "sarif", "--max-show-issues", "1"][..],
        &["--format", "github", "--max-show-issues", "1"],
        &["--fix", "--max-show-issues", "1"],
    ] {
        let dir = fixture("filters");
        let result = run(skif().args(args).arg(dir.path()));
        assert_eq!(result.code, 2, "{args:?}: {}{}", result.stdout, result.stderr);
        assert!(
            result.stderr.contains("--max-show-issues"),
            "{args:?}: {}",
            result.stderr
        );
    }
}

#[test]
fn trace_dependency_conflicts_with_the_report_filters() {
    for filter in [
        "--workspace=@cli/a",
        "--include=files",
        "--exclude=files",
        "--files",
        "--dependencies",
    ] {
        let (result, error) = lint_json(&["--trace-dependency", "left-pad", filter]);
        assert_eq!(result.code, 2, "{filter}");
        assert_eq!(error["error"]["kind"], "usage", "{filter}: {error:#}");
    }
}
