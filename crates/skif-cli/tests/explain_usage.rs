//! End-to-end tests of `skif explain-usage`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fs;
use std::path::Path;

use support::{redact, run, skif};
use tempfile::TempDir;

fn put(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(
        dir.path(),
        "package.json",
        r#"{"name":"app","version":"1.0.0","private":true,"main":"index.ts","devDependencies":{"vitest":"3.0.0"}}"#,
    );
    put(
        dir.path(),
        "index.ts",
        "import { go } from './barrel';\nimport './nested/leaf';\nexport const main = go();\n",
    );
    put(dir.path(), "barrel.ts", "export { run as go } from './lib';\n");
    put(
        dir.path(),
        "lib.ts",
        "export function run() {}\nexport function idle() {}\n",
    );
    put(dir.path(), "nested/leaf.ts", "export const leaf = 1;\n");
    put(dir.path(), "orphan.ts", "export const lonely = 1;\n");
    put(dir.path(), "a.test.ts", "import './lib';\n");
    dir
}

#[test]
fn a_used_export_is_explained_in_human_output() {
    let dir = project();
    let result = run(skif().current_dir(dir.path()).args(["explain-usage", "lib.ts#run"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    insta::assert_snapshot!("explain_usage_human", redact(&result.stdout, dir.path()));
}

#[test]
fn an_unused_export_is_explained_in_json() {
    let dir = project();
    let result = run(skif()
        .current_dir(dir.path())
        .args(["explain-usage", "lib.ts#idle", "--format", "json"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let document = result.json();
    assert_eq!(document["report"]["export"]["verdict"], "unused");
    insta::assert_snapshot!("explain_usage_json", redact(&result.stdout, dir.path()));
}

#[test]
fn an_entry_exports_are_public_and_the_verdict_agrees() {
    let dir = project();
    let result = run(skif()
        .current_dir(dir.path())
        .args(["explain-usage", "index.ts#main", "--format", "json"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let document = result.json();
    assert!(document["report"]["entry"].is_string(), "{document}");
    assert_eq!(document["report"]["export"]["entry-public"], true);
    assert_eq!(document["report"]["export"]["verdict"], "public");
}

#[test]
fn a_file_outside_the_graph_exits_two_with_the_reason() {
    let dir = project();
    let human = run(skif().current_dir(dir.path()).args(["explain-usage", "orphan.ts"]));
    assert_eq!(human.code, 2);
    assert_eq!(human.stdout, "");
    assert!(human.stderr.contains("unused-files"), "{}", human.stderr);

    let json = run(skif()
        .current_dir(dir.path())
        .args(["explain-usage", "orphan.ts", "--format", "json"]));
    assert_eq!(json.code, 2);
    let document = json.json();
    assert_eq!(document["error"]["kind"], "usage");
    assert_eq!(document["report"]["absence"], "unused-file");
}

#[test]
fn a_missing_file_exits_two() {
    let dir = project();
    let result = run(skif().current_dir(dir.path()).args(["explain-usage", "nope.ts"]));
    assert_eq!(result.code, 2);
    assert!(result.stderr.starts_with("error: "), "{}", result.stderr);
}

#[test]
fn the_file_is_resolved_from_the_working_directory() {
    let dir = project();
    let nested = dir.path().join("nested");
    let result = run(skif()
        .current_dir(&nested)
        .args(["explain-usage", "leaf.ts", "..", "--format", "json"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.json()["report"]["file"], "nested/leaf.ts");

    let absolute = run(skif()
        .current_dir(&nested)
        .arg("explain-usage")
        .arg(dir.path().join("lib.ts"))
        .args(["--format", "json"]));
    assert_eq!(absolute.code, 0, "{}{}", absolute.stdout, absolute.stderr);
    assert_eq!(absolute.json()["report"]["file"], "lib.ts");
}

#[test]
fn production_removes_test_entries() {
    let dir = project();
    let default = run(skif().current_dir(dir.path()).args(["explain-usage", "a.test.ts"]));
    assert_eq!(default.code, 0, "{}{}", default.stdout, default.stderr);
    let production = run(skif()
        .current_dir(dir.path())
        .args(["explain-usage", "a.test.ts", "--production"]));
    assert_eq!(production.code, 2, "{}{}", production.stdout, production.stderr);
}

#[test]
fn an_export_in_a_package_with_incomplete_entries_is_not_judged() {
    let dir = project();
    // `dist/server.js` maps to no source file, so the package's real entries are unknown.
    put(
        dir.path(),
        "package.json",
        r#"{"name":"app","version":"1.0.0","private":true,"main":"./dist/server.js","devDependencies":{"vitest":"3.0.0"}}"#,
    );
    let json = run(skif()
        .current_dir(dir.path())
        .args(["explain-usage", "lib.ts#idle", "--format", "json"]));
    assert_eq!(json.code, 0, "{}{}", json.stdout, json.stderr);
    let document = json.json();
    assert_eq!(document["report"]["export"]["verdict"], "not-judged", "{document}");
    assert_eq!(document["report"]["export"]["entries-incomplete"], true, "{document}");

    let human = run(skif().current_dir(dir.path()).args(["explain-usage", "lib.ts#idle"]));
    assert_eq!(human.code, 0, "{}{}", human.stdout, human.stderr);
    assert!(human.stdout.contains("=> not judged"), "{}", human.stdout);
}
