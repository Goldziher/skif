//! End-to-end tests of the `skif` binary against the contract in ADR 0016.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{changed, fixture, invocations, redact, run, shim, skif, tree};

fn rule_ids(report: &Value) -> BTreeSet<String> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["rule"].as_str().unwrap().to_owned())
        .collect()
}

fn paths(items: &Value) -> BTreeSet<PathBuf> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| PathBuf::from(item.as_str().unwrap()))
        .collect()
}

fn expected_fixed() -> BTreeSet<PathBuf> {
    BTreeSet::from([
        PathBuf::from("package.json"),
        Path::new("packages").join("a").join("package.json"),
    ])
}

#[test]
fn clean_workspace_exits_zero() {
    let dir = fixture("clean");
    let human = run(skif().arg(dir.path()));
    assert_eq!(human.code, 0, "{}{}", human.stdout, human.stderr);
    insta::assert_snapshot!("clean_human", redact(&human.stdout, dir.path()));

    let json = run(skif().arg("--format").arg("json").arg(dir.path()));
    assert_eq!(json.code, 0);
    assert_eq!(json.json()["summary"]["errors"], 0);
    insta::assert_snapshot!("clean_json", redact(&json.stdout, dir.path()));
}

#[test]
fn issues_exit_one_in_both_formats() {
    let dir = fixture("dirty");
    let human = run(skif().arg(dir.path()));
    assert_eq!(human.code, 1);
    insta::assert_snapshot!("dirty_human", redact(&human.stdout, dir.path()));

    let json = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(json.code, 1);
    let report = json.json();
    assert!(rule_ids(&report).is_superset(&BTreeSet::from([
        "empty-dependencies".to_owned(),
        "root-package-private-field".to_owned(),
    ])));
    insta::assert_snapshot!("dirty_json", redact(&json.stdout, dir.path()));
}

#[test]
fn a_directory_inside_the_workspace_lints_the_whole_workspace() {
    let dir = fixture("dirty");
    let inside = dir.path().join("packages").join("a");
    let from_root = run(skif().args(["--format", "json"]).arg(dir.path()));
    let from_member = run(skif().args(["--format", "json"]).arg(&inside));
    assert_eq!(from_member.code, 1);
    assert_eq!(from_member.json(), from_root.json());
}

#[test]
fn invalid_config_exits_two_in_both_formats() {
    let dir = fixture("clean");
    fs::write(dir.path().join("skif.toml"), "[lint]\nbogus = true\n").unwrap();

    let human = run(skif().arg(dir.path()));
    assert_eq!(human.code, 2);
    assert!(human.stdout.is_empty(), "{}", human.stdout);
    insta::assert_snapshot!("config_error_human", redact(&human.stderr, dir.path()));

    let json = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(json.code, 2);
    assert!(json.stderr.is_empty(), "{}", json.stderr);
    let document = json.json();
    assert_eq!(document["error"]["kind"], "config");
    assert_eq!(document["error"]["line"], 2);
    assert!(Path::new(document["error"]["path"].as_str().unwrap()).ends_with("skif.toml"));
    insta::assert_snapshot!("config_error_json", redact(&json.stdout, dir.path()));
}

#[test]
fn unknown_rule_in_config_exits_two() {
    let dir = fixture("clean");
    fs::write(dir.path().join("skif.toml"), "[rules]\nno-such-rule = \"error\"\n").unwrap();
    let result = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(result.code, 2);
    assert_eq!(result.json()["error"]["kind"], "config");
}

#[test]
fn both_config_files_exit_two() {
    let dir = fixture("clean");
    fs::write(dir.path().join("skif.toml"), "").unwrap();
    fs::write(dir.path().join(".skif.toml"), "").unwrap();
    let result = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(result.code, 2);
    assert_eq!(result.json()["error"]["kind"], "config");
}

#[test]
fn a_directory_without_a_workspace_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let json = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(json.code, 2);
    assert_eq!(json.json()["error"]["kind"], "workspace");
    let human = run(skif().arg(dir.path()));
    assert_eq!(human.code, 2);
    assert_eq!(human.stdout, "");
    assert!(human.stderr.starts_with("error:"), "{}", human.stderr);
}

#[test]
fn usage_errors_exit_two_and_are_json_in_json_mode() {
    let human = run(skif().arg("--no-such-flag"));
    assert_eq!(human.code, 2);
    assert_eq!(human.stdout, "");

    let json = run(skif().args(["--format", "json", "--no-such-flag"]));
    assert_eq!(json.code, 2);
    let document = json.json();
    assert_eq!(document["error"]["kind"], "usage");
    assert_eq!(document["version"], 1);

    let equals_form = run(skif().args(["--format=json", "--select", "middle"]));
    assert_eq!(equals_form.code, 2);
    assert_eq!(equals_form.json()["error"]["kind"], "usage");
}

#[test]
fn output_requires_a_format() {
    let dir = fixture("clean");
    let result = run(skif().arg("--output").arg(dir.path().join("out.txt")).arg(dir.path()));
    assert_eq!(result.code, 2);
}

#[test]
fn output_writes_the_file_and_keeps_stdout_empty() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("report.json");
    let result = run(skif()
        .args(["--format", "json", "--output"])
        .arg(&target)
        .arg(dir.path()));
    assert_eq!(result.code, 1);
    assert!(result.stdout.is_empty(), "{}", result.stdout);
    let document: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(document["version"], 1);
}

#[test]
fn failing_run_does_not_write_output_and_reports_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("report.json");
    let result = run(skif()
        .args(["--format", "json", "--output"])
        .arg(&target)
        .arg(dir.path()));
    assert_eq!(result.code, 2);
    assert!(!target.exists());
    assert_eq!(result.json()["error"]["kind"], "workspace");
}

#[test]
fn max_warnings_decides_between_zero_and_one() {
    let dir = fixture("dirty");
    let base = ["--rule", "root-package-private-field=off"];
    let cases: [(&[&str], i32); 5] = [
        (&[], 0),
        (&["--max-warnings", "0"], 1),
        (&["--fail-on-warnings"], 1),
        (&["--max-warnings", "1"], 0),
        (&["--max-warnings", "99"], 0),
    ];
    for (extra, code) in cases {
        let result = run(skif().args(base).args(extra).arg(dir.path()));
        assert_eq!(result.code, code, "{extra:?}\n{}{}", result.stdout, result.stderr);
    }
}

#[test]
fn rule_levels_and_ignores_change_the_outcome() {
    let dir = fixture("dirty");
    let ignored = run(skif()
        .args([
            "--rule",
            "root-package-private-field=off",
            "--ignore-rule",
            "empty-dependencies",
        ])
        .arg(dir.path()));
    assert_eq!(ignored.code, 0, "{}{}", ignored.stdout, ignored.stderr);

    let package = run(skif()
        .args(["--rule", "root-package-private-field=off", "--ignore-package", "@cli/a"])
        .arg(dir.path()));
    assert_eq!(package.code, 0, "{}{}", package.stdout, package.stderr);

    let scoped = run(skif()
        .args([
            "--rule",
            "root-package-private-field=off",
            "--ignore-rule-for-package",
            "empty-dependencies:@cli/a",
        ])
        .arg(dir.path()));
    assert_eq!(scoped.code, 0, "{}{}", scoped.stdout, scoped.stderr);

    let unknown = run(skif().args(["--ignore-rule", "no-such-rule"]).arg(dir.path()));
    assert_eq!(unknown.code, 2);
}

#[test]
fn fix_from_another_directory_writes_root_relative_files_only() {
    let dir = fixture("dirty");
    let elsewhere = tempfile::tempdir().unwrap();
    let before = tree(dir.path());

    let result = run(skif()
        .current_dir(elsewhere.path())
        .args(["--fix", "--no-install", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);

    let after = tree(dir.path());
    assert_eq!(
        changed(&before, &after).into_iter().collect::<BTreeSet<_>>(),
        expected_fixed()
    );
    assert!(
        tree(elsewhere.path()).is_empty(),
        "files leaked into the current directory"
    );

    let root: Value = serde_json::from_str(&after[Path::new("package.json")]).unwrap();
    assert_eq!(root["private"], true);
    let member: Value = serde_json::from_str(&after[&Path::new("packages").join("a").join("package.json")]).unwrap();
    assert!(member.get("devDependencies").is_none(), "{member}");

    let report = result.json();
    assert_eq!(paths(&report["fix"]["changed"]), expected_fixed());
    assert_eq!(report["fix"]["install"]["status"], "skipped");
    insta::assert_snapshot!("fix_json", redact(&result.stdout, dir.path()));
}

#[test]
fn fix_started_inside_a_member_still_fixes_the_root() {
    let dir = fixture("dirty");
    let member = dir.path().join("packages").join("a");
    let before = tree(dir.path());
    let result = run(skif().current_dir(&member).args(["--fix", "--no-install"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert_eq!(
        changed(&before, &tree(dir.path())).into_iter().collect::<BTreeSet<_>>(),
        expected_fixed()
    );
    assert!(tree(&member).keys().all(|path| path == Path::new("package.json")));
}

#[test]
fn fix_is_refused_only_when_ci_is_truthy() {
    let cases = [
        (None, true),
        (Some(""), true),
        (Some("0"), true),
        (Some("false"), true),
        (Some("FALSE"), true),
        (Some("no"), true),
        (Some("off"), true),
        (Some("1"), false),
        (Some("true"), false),
        (Some("yes"), false),
    ];
    for (ci, allowed) in cases {
        let dir = fixture("dirty");
        let before = tree(dir.path());
        let mut command = skif();
        command
            .args(["--fix", "--no-install", "--format", "json"])
            .arg(dir.path());
        if let Some(value) = ci {
            command.env("CI", value);
        }
        let result = run(&mut command);
        if allowed {
            assert_eq!(result.code, 0, "CI={ci:?}\n{}{}", result.stdout, result.stderr);
            assert_ne!(tree(dir.path()), before, "CI={ci:?}");
        } else {
            assert_eq!(result.code, 2, "CI={ci:?}");
            assert_eq!(result.json()["error"]["kind"], "ci-refusal", "CI={ci:?}");
            assert_eq!(tree(dir.path()), before, "CI={ci:?}");
        }
    }
}

#[test]
fn ci_does_not_affect_linting() {
    let dir = fixture("clean");
    let result = run(skif().env("CI", "true").arg(dir.path()));
    assert_eq!(result.code, 0);
}

#[test]
fn fix_unsafe_implies_fix() {
    let dir = fixture("dirty");
    let before = tree(dir.path());
    let result = run(skif().args(["--fix-unsafe", "--no-install"]).arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert_ne!(tree(dir.path()), before);
}

#[test]
fn install_runs_once_in_the_workspace_root_after_a_fix() {
    let dir = fixture("dirty");
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let bin = shim("npm", &record, 0);
    let elsewhere = tempfile::tempdir().unwrap();

    let result = run(skif()
        .env("PATH", bin.path())
        .current_dir(elsewhere.path())
        .args(["--fix", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);

    let calls = invocations(&record);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, "install");
    assert_eq!(calls[0].1.canonicalize().unwrap(), dir.path().canonicalize().unwrap());
    assert_eq!(result.json()["fix"]["install"]["status"], "ran");
}

#[test]
fn install_is_not_run_without_fix_without_changes_or_with_no_install() {
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let bin = shim("npm", &record, 0);

    let dirty = fixture("dirty");
    let lint_only = run(skif().env("PATH", bin.path()).arg(dirty.path()));
    assert_eq!(lint_only.code, 1);

    let opted_out = run(skif()
        .env("PATH", bin.path())
        .args(["--fix", "--no-install"])
        .arg(dirty.path()));
    assert_eq!(opted_out.code, 0);

    let clean = fixture("clean");
    let nothing_to_fix = run(skif().env("PATH", bin.path()).arg("--fix").arg(clean.path()));
    assert_eq!(nothing_to_fix.code, 0);

    assert!(invocations(&record).is_empty(), "{:?}", invocations(&record));
}

#[test]
fn install_is_skipped_when_no_package_manager_is_detected() {
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let bin = shim("npm", &record, 0);
    let dir = fixture("dirty");
    let manifest = dir.path().join("package.json");
    let without_manager = fs::read_to_string(&manifest)
        .unwrap()
        .replace("  \"packageManager\": \"npm@11.6.2\",\n", "");
    fs::write(&manifest, without_manager).unwrap();

    let result = run(skif()
        .env("PATH", bin.path())
        .args(["--fix", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.json()["fix"]["install"]["status"], "skipped");
    assert_eq!(invocations(&record).len(), 0);
}

#[test]
fn failed_install_exits_two_but_keeps_the_fixes() {
    let dir = fixture("dirty");
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let bin = shim("npm", &record, 1);
    let before = tree(dir.path());

    let result = run(skif()
        .env("PATH", bin.path())
        .args(["--fix", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.json()["fix"]["install"]["status"], "failed");
    assert_eq!(invocations(&record).len(), 1);
    assert_ne!(tree(dir.path()), before);
}

#[test]
fn missing_package_manager_is_a_failed_install() {
    let dir = fixture("dirty");
    let empty_path = tempfile::tempdir().unwrap();
    let result = run(skif()
        .env("PATH", empty_path.path())
        .args(["--fix", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 2);
    assert_eq!(result.json()["fix"]["install"]["status"], "failed");
}

#[test]
fn rules_lists_the_registry() {
    let human = run(skif().arg("rules"));
    assert_eq!(human.code, 0);
    assert!(human.stdout.starts_with("RULE"), "{}", human.stdout);
    insta::assert_snapshot!("rules_human", human.stdout);

    let json = run(skif().args(["rules", "--format", "json"]));
    assert_eq!(json.code, 0);
    let document = json.json();
    let rules = document["rules"].as_array().unwrap();
    assert_ne!(rules.len(), 0);
    for rule in rules {
        for key in ["id", "category", "default", "fixable", "summary"] {
            assert!(rule.get(key).is_some(), "{rule} lacks {key}");
        }
    }
    let ids: BTreeSet<&str> = rules.iter().map(|rule| rule["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), rules.len(), "rule ids are unique");
    insta::assert_snapshot!("rules_json", json.stdout);
}

#[test]
fn schema_prints_the_config_schema() {
    let result = run(skif().arg("schema"));
    assert_eq!(result.code, 0);
    let schema = result.json();
    assert!(schema["properties"].get("lint").is_some(), "{schema}");
    assert!(schema["properties"].get("rules").is_some());
}

#[test]
fn init_writes_a_starter_config_and_refuses_to_overwrite() {
    let dir = fixture("clean");
    let created = run(skif().arg("init").arg(dir.path()));
    assert_eq!(created.code, 0, "{}{}", created.stdout, created.stderr);
    let target = dir.path().join("skif.toml");
    let starter = fs::read_to_string(&target).unwrap();
    assert!(starter.starts_with("#:schema "), "{starter}");
    insta::assert_snapshot!("init_config", starter);

    let linted = run(skif().arg(dir.path()));
    assert_eq!(
        linted.code, 0,
        "the starter config must be valid: {}{}",
        linted.stdout, linted.stderr
    );

    fs::write(&target, "# mine\n").unwrap();
    let refused = run(skif().arg("init").arg(dir.path()));
    assert_eq!(refused.code, 2);
    assert_eq!(fs::read_to_string(&target).unwrap(), "# mine\n");
    insta::assert_snapshot!("init_refused", redact(&refused.stderr, dir.path()));

    fs::remove_file(&target).unwrap();
    fs::write(dir.path().join(".skif.toml"), "").unwrap();
    let hidden = run(skif().arg("init").arg(dir.path()));
    assert_eq!(hidden.code, 2);
    assert!(!target.exists());
}

#[test]
fn help_and_version_exit_zero() {
    let help = run(skif().arg("--help"));
    assert_eq!(help.code, 0);
    insta::assert_snapshot!("help", help.stdout);
    let version = run(skif().arg("--version"));
    assert_eq!(version.code, 0);
    assert!(version.stdout.starts_with("skif "), "{}", version.stdout);
}
