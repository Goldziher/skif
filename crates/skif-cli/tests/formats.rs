//! Several reports in one run: a repeatable `--format FORMAT[=FILE]` (ADR 0022).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fs;

use serde_json::Value;
use support::{fixture, run, skif};

#[test]
fn several_formats_go_to_their_files_and_one_to_stdout() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let json_file = out.path().join("skif.json");
    let codeclimate_file = out.path().join("gl-code-quality.json");
    let result = run(skif()
        .args(["--format", "human", "--format"])
        .arg(format!("json={}", json_file.display()))
        .arg(format!("--format=codeclimate={}", codeclimate_file.display()))
        .arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    let alone = run(skif().arg(dir.path()));
    assert_eq!(result.stdout, alone.stdout);
    assert!(!result.stdout.contains('\x1b'), "{}", result.stdout);

    let json = run(skif().args(["--format", "json"]).arg(dir.path())).json();
    let from_file: Value = serde_json::from_str(&fs::read_to_string(&json_file).unwrap()).unwrap();
    assert_eq!(from_file, json);
    let codeclimate = run(skif().args(["--format", "codeclimate"]).arg(dir.path())).json();
    let from_file: Value = serde_json::from_str(&fs::read_to_string(&codeclimate_file).unwrap()).unwrap();
    assert_eq!(from_file, codeclimate);
}

#[test]
fn only_files_leave_stdout_empty() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let sarif_file = out.path().join("skif.sarif");
    let markdown_file = out.path().join("skif.md");
    let result = run(skif()
        .arg(format!("--format=sarif={}", sarif_file.display()))
        .arg(format!("--format=markdown={}", markdown_file.display()))
        .arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.is_empty(), "{}", result.stdout);
    let log: Value = serde_json::from_str(&fs::read_to_string(&sarif_file).unwrap()).unwrap();
    assert_eq!(log["version"], "2.1.0");
    assert!(
        fs::read_to_string(&markdown_file)
            .unwrap()
            .starts_with("# skif report\n")
    );
}

#[test]
fn two_formats_on_stdout_are_a_usage_error() {
    let dir = fixture("dirty");
    let human = run(skif()
        .args(["--format", "human", "--format", "compact"])
        .arg(dir.path()));
    assert_eq!(human.code, 2);
    assert!(human.stdout.is_empty(), "{}", human.stdout);
    assert!(human.stderr.contains("at most one --format"), "{}", human.stderr);

    let json = run(skif().args(["--format", "json", "--format", "sarif"]).arg(dir.path()));
    assert_eq!(json.code, 2);
    assert_eq!(json.json()["error"]["kind"], "usage");
}

#[test]
fn output_with_several_formats_is_a_usage_error() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("report.json");
    let several = run(skif()
        .args(["--format", "json", "--format"])
        .arg(format!("sarif={}", out.path().join("skif.sarif").display()))
        .arg("--output")
        .arg(&target)
        .arg(dir.path()));
    assert_eq!(several.code, 2);
    assert_eq!(several.json()["error"]["kind"], "usage");
    let with_file = run(skif()
        .arg(format!("--format=json={}", out.path().join("other.json").display()))
        .arg("--output")
        .arg(&target)
        .arg(dir.path()));
    assert_eq!(with_file.code, 2);
    let error = with_file.json();
    assert!(
        error["error"]["message"].as_str().unwrap().contains("--output"),
        "{error}"
    );
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn the_same_file_twice_is_a_usage_error() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("report");
    let result = run(skif()
        .arg(format!("--format=json={}", target.display()))
        .arg(format!("--format=sarif={}", target.display()))
        .arg(dir.path()));
    assert_eq!(result.code, 2);
    assert!(!target.exists());
}

#[test]
fn the_same_file_spelled_twice_is_a_usage_error() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("report");
    let result = run(skif()
        .arg(format!(
            "--format=json={}",
            out.path().join("missing/../report").display()
        ))
        .arg(format!("--format=sarif={}", out.path().join("./report").display()))
        .arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.json()["error"]["kind"], "usage");
    assert!(!target.exists());

    fs::create_dir(out.path().join("sub")).unwrap();
    let result = run(skif()
        .arg(format!("--format=json={}", out.path().join("sub/../report").display()))
        .arg(format!("--format=sarif={}", target.display()))
        .arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(!target.exists());
}

#[test]
fn a_directory_target_is_a_usage_error_before_any_write() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let json_file = out.path().join("skif.json");
    let result = run(skif()
        .arg(format!("--format=json={}", json_file.display()))
        .arg(format!("--format=sarif={}", out.path().display()))
        .arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.json()["error"]["kind"], "usage");
    assert!(!json_file.exists());
}

#[test]
fn only_the_stdout_rendering_is_coloured() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("report.txt");
    let result = run(skif()
        .args(["--color", "always", "--format", "human"])
        .arg(format!("--format=human={}", file.display()))
        .arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.contains('\x1b'), "{}", result.stdout);
    assert!(!fs::read_to_string(&file).unwrap().contains('\x1b'));
}

#[test]
fn a_failing_render_writes_no_file() {
    let dir = fixture("dirty");
    let out = tempfile::tempdir().unwrap();
    let json_file = out.path().join("skif.json");
    let owners_file = out.path().join("owners.txt");
    let result = run(skif()
        .arg(format!("--format=json={}", json_file.display()))
        .arg(format!("--format=codeowners={}", owners_file.display()))
        .arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(!json_file.exists());
    assert!(!owners_file.exists());
    // No format goes to stdout, so the first one decides: the error is a JSON document.
    assert_eq!(result.json()["error"]["kind"], "config");
}

#[test]
fn the_stdout_format_decides_json_errors() {
    let out = tempfile::tempdir().unwrap();
    let report = out.path().join("report.json");
    let human = run(skif()
        .arg(format!("--format=json={}", report.display()))
        .args(["--format", "human"])
        .arg(out.path()));
    assert_eq!(human.code, 2);
    assert!(human.stdout.is_empty(), "{}", human.stdout);
    assert!(human.stderr.starts_with("error:"), "{}", human.stderr);

    let json =
        run(skif()
            .arg(format!("--format=sarif={}", report.display()))
            .args(["--format", "json", "--no-such-flag"]));
    assert_eq!(json.code, 2);
    assert_eq!(json.json()["error"]["kind"], "usage");
    assert!(!report.exists());
}

#[test]
fn an_unknown_format_or_an_empty_file_is_a_usage_error() {
    let unknown = run(skif().args(["--format", "yaml"]));
    assert_eq!(unknown.code, 2);
    assert!(unknown.stderr.contains("unknown format `yaml`"), "{}", unknown.stderr);
    let empty = run(skif().args(["--format", "json="]));
    assert_eq!(empty.code, 2);
    assert_eq!(empty.json()["error"]["kind"], "usage");
}
