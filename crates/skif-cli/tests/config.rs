//! End-to-end tests of `skif config` and of the inheritance note of `skif init`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fs;
use std::path::Path;

use serde_json::Value;
use support::{run, skif};
use tempfile::TempDir;

fn put(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A repository whose `ws` workspace is configured by an org file above it, a preset it extends and its own file.
fn chain() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(
        dir.path(),
        "skif.toml",
        "[lint]\nfail-on-warnings = true\n\n[[overrides]]\npackages = [\"@acme/*\"]\nmax-level = \"warn\"\n",
    );
    put(
        dir.path(),
        "presets/base.toml",
        "# shared preset\n[lint]\nfail-on-warnings = false\n\n[rules]\nunsorted-dependencies = \"warn\"\n",
    );
    put(
        dir.path(),
        "ws/skif.toml",
        "extends = [\"../presets/base.toml\"]\n\n[lint]\nfail-on-warnings = true\n\n[rules]\nunsorted-dependencies = \"off\"\n",
    );
    put(
        dir.path(),
        "ws/package.json",
        r#"{"name":"ws","version":"0.0.0","private":true,"packageManager":"npm@11.6.2","workspaces":["packages/*"]}"#,
    );
    put(
        dir.path(),
        "ws/packages/a/package.json",
        r#"{"name":"@acme/a","version":"1.0.0"}"#,
    );
    put(dir.path(), "ws/packages/b/package.json", r#"{"name":"b","version":"1.0.0"}"#);
    dir
}

/// Two independent projects, each with its own configuration file.
fn two_roots() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(dir.path(), "one/package.json", r#"{"name":"one","version":"1.0.0"}"#);
    put(dir.path(), "one/skif.toml", "[lint]\nfail-on-warnings = true\n");
    put(dir.path(), "two/package.json", r#"{"name":"two","version":"1.0.0"}"#);
    put(dir.path(), "two/skif.toml", "[policy]\nprefer = \"lowest\"\n");
    dir
}

fn paths(sources: &Value) -> Vec<&str> {
    sources
        .as_array()
        .unwrap()
        .iter()
        .map(|source| source["path"].as_str().unwrap())
        .collect()
}

#[test]
fn show_lists_the_sources_in_apply_order_then_the_merged_config() {
    let dir = chain();
    let ws = dir.path().join("ws");
    let result = run(skif().args(["config", "--show"]).arg(&ws));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let header: Vec<&str> = result.stdout.lines().filter(|line| line.starts_with('#')).collect();
    insta::assert_snapshot!("show_sources", header.join("\n"));
    assert!(result.stdout.contains("fail-on-warnings = true"), "{}", result.stdout);
}

#[test]
fn show_json_has_the_envelope_the_sources_and_the_config() {
    let dir = chain();
    let result = run(skif().args(["config", "--show", "--format", "json"]).arg(dir.path().join("ws")));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let document = result.json();
    assert_eq!(document["version"], 1);
    assert!(document["skif"].is_string());
    let roots = document["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0]["path"], ".");
    let sources = &roots[0]["sources"];
    assert_eq!(paths(sources), ["../skif.toml", "../presets/base.toml", "skif.toml"]);
    let kinds: Vec<&str> = sources
        .as_array()
        .unwrap()
        .iter()
        .map(|source| source["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["ancestor", "extends", "root"]);
    assert_eq!(sources[1]["extended-by"], "skif.toml");
    assert_eq!(roots[0]["config"]["lint"]["fail-on-warnings"], true);
}

#[test]
fn explain_walks_the_whole_chain() {
    let dir = chain();
    let ws = dir.path().join("ws");
    let human = run(skif().args(["config", "--explain", "lint.fail-on-warnings"]).arg(&ws));
    assert_eq!(human.code, 0, "{}{}", human.stdout, human.stderr);
    insta::assert_snapshot!("explain_human", human.stdout);

    let json = run(skif()
        .args(["config", "--explain", "lint.fail-on-warnings", "--format", "json"])
        .arg(&ws));
    assert_eq!(json.code, 0, "{}{}", json.stdout, json.stderr);
    insta::assert_snapshot!("explain_json", json.stdout);
}

#[test]
fn explain_a_rule_level_lists_each_file_that_set_it() {
    let dir = chain();
    let result = run(skif()
        .args(["config", "--explain", "rules.unsorted-dependencies.level", "--format", "json"])
        .arg(dir.path().join("ws")));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let document = result.json();
    let entry = &document["roots"][0]["entries"][0];
    assert_eq!(entry["value"], "off");
    let set_by = entry["set-by"].as_array().unwrap();
    assert_eq!(paths(&entry["set-by"]), ["../presets/base.toml", "skif.toml"]);
    assert!(set_by.iter().all(|located| located["op"] == "set"));
}

#[test]
fn explain_with_a_package_lists_the_overrides_that_apply() {
    let dir = chain();
    let ws = dir.path().join("ws");
    let result = run(skif()
        .args([
            "config",
            "--explain",
            "lint.fail-on-warnings",
            "--package",
            "@acme/a",
            "--format",
            "json",
        ])
        .arg(&ws));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let document = result.json();
    let package = &document["roots"][0]["package"];
    assert_eq!(package["name"], "@acme/a");
    assert_eq!(package["dir"], "packages/a");
    let overrides = package["overrides"].as_array().unwrap();
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0]["index"], 0);
    assert_eq!(overrides[0]["path"], "../skif.toml");

    let human = run(skif()
        .args(["config", "--explain", "lint.fail-on-warnings", "--package", "@acme/a"])
        .arg(&ws));
    assert!(human.stdout.contains("package @acme/a (packages/a)"), "{}", human.stdout);
    assert!(human.stdout.contains("overrides[0]  ../skif.toml"), "{}", human.stdout);

    let other = run(skif()
        .args(["config", "--explain", "lint.fail-on-warnings", "--package", "b"])
        .arg(&ws));
    assert!(other.stdout.contains("package b (packages/b)"), "{}", other.stdout);
    assert!(other.stdout.contains("    none"), "{}", other.stdout);
}

#[test]
fn an_unknown_key_exits_two_and_names_near_keys() {
    let dir = chain();
    let ws = dir.path().join("ws");
    let human = run(skif().args(["config", "--explain", "lint.fail-on-warning"]).arg(&ws));
    assert_eq!(human.code, 2);
    assert!(
        human.stderr.contains("unknown configuration key `lint.fail-on-warning`"),
        "{}",
        human.stderr
    );
    assert!(human.stderr.contains("`lint.fail-on-warnings`"), "{}", human.stderr);

    let json = run(skif()
        .args(["config", "--explain", "lint.fail-on-warning", "--format", "json"])
        .arg(&ws));
    assert_eq!(json.code, 2);
    assert_eq!(json.json()["error"]["kind"], "usage");
}

#[test]
fn an_unknown_package_exits_two() {
    let dir = chain();
    let result = run(skif()
        .args(["config", "--explain", "lint.fail-on-warnings", "--package", "nope"])
        .arg(dir.path().join("ws")));
    assert_eq!(result.code, 2);
    assert!(result.stderr.contains("no package named `nope`"), "{}", result.stderr);
}

#[test]
fn show_or_explain_is_required_and_they_conflict() {
    let dir = chain();
    let ws = dir.path().join("ws");
    assert_eq!(run(skif().arg("config").arg(&ws)).code, 2);
    assert_eq!(
        run(skif().args(["config", "--show", "--explain", "lint.fail-on-warnings"]).arg(&ws)).code,
        2
    );
    assert_eq!(
        run(skif().args(["config", "--package", "a", "--show"]).arg(&ws)).code,
        2
    );
}

#[test]
fn recursive_reports_every_root_with_its_own_sources() {
    let dir = two_roots();
    let result = run(skif()
        .args(["config", "-r", "--show", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let document = result.json();
    let roots = document["roots"].as_array().unwrap();
    let names: Vec<&str> = roots.iter().map(|root| root["path"].as_str().unwrap()).collect();
    assert_eq!(names, ["one", "two"]);
    assert_eq!(paths(&roots[0]["sources"]), ["one/skif.toml"]);
    assert_eq!(paths(&roots[1]["sources"]), ["two/skif.toml"]);
    assert_eq!(roots[0]["sources"][0]["kind"], "root");
    assert_eq!(roots[0]["config"]["lint"]["fail-on-warnings"], true);
    assert_eq!(roots[1]["config"]["policy"]["prefer"], "lowest");

    let explained = run(skif()
        .args(["config", "-r", "--explain", "policy.prefer"])
        .arg(dir.path()));
    assert_eq!(explained.code, 0, "{}{}", explained.stdout, explained.stderr);
    assert!(explained.stdout.contains("# root: one"), "{}", explained.stdout);
    assert!(explained.stdout.contains("# root: two"), "{}", explained.stdout);
    assert!(explained.stdout.contains("two/skif.toml:2:1  set"), "{}", explained.stdout);
}

#[test]
fn init_names_the_files_the_new_one_inherits_from() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(dir.path(), "skif.toml", "[lint]\nfail-on-warnings = true\n");
    let sub = dir.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    let result = run(skif().arg("init").arg(&sub));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.starts_with("Created "), "{}", result.stdout);
    assert!(
        result.stdout.contains("Note: it will inherit from ../skif.toml\n"),
        "{}",
        result.stdout
    );
}
