//! `skif migrate`: discovery, output handling and the exit codes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use support::{run, skif};

const SHERIF_PACKAGE: &str = r#"{
  "name": "demo",
  "private": true,
  "scripts": { "check": "sherif --fail-on-warnings" },
  "sherif": { "ignoreRule": ["unordered-dependencies"], "select": "lowest" }
}"#;

fn project(package_json: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("package.json"), package_json).unwrap();
    dir
}

fn migrate(dir: &Path) -> assert_cmd::Command {
    let mut command = skif();
    command.arg("migrate").arg(dir);
    command
}

#[test]
fn dry_run_prints_the_configuration_and_writes_nothing() {
    let dir = project(SHERIF_PACKAGE);
    let result = run(migrate(dir.path()).args(["--dry-run", "--profile", "full"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.starts_with("#:schema "), "{}", result.stdout);
    assert!(
        result.stdout.contains("[lint]\nfail-on-warnings = true"),
        "{}",
        result.stdout
    );
    assert!(
        result.stdout.contains("unsorted-dependencies = \"off\""),
        "{}",
        result.stdout
    );
    assert!(result.stderr.contains("Translated"), "{}", result.stderr);
    assert!(!dir.path().join("skif.toml").exists());
}

#[test]
fn the_written_file_is_valid_and_is_not_overwritten_without_a_flag() {
    let dir = project(SHERIF_PACKAGE);
    let written = run(&mut migrate(dir.path()));
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    assert!(written.stdout.contains("Wrote "), "{}", written.stdout);
    let target = dir.path().join("skif.toml");
    let first = fs::read_to_string(&target).unwrap();
    assert!(
        first.contains("# Migrated from sherif by `skif migrate` (profile: exact)."),
        "{first}"
    );

    let refused = run(&mut migrate(dir.path()));
    assert_eq!(refused.code, 2, "{}{}", refused.stdout, refused.stderr);
    assert!(refused.stderr.contains("already exists"), "{}", refused.stderr);
    assert_eq!(fs::read_to_string(&target).unwrap(), first);

    let merged = run(migrate(dir.path()).arg("--merge"));
    assert_eq!(merged.code, 0, "{}{}", merged.stdout, merged.stderr);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        first,
        "merging the same migration changes nothing"
    );

    let linted = run(skif().arg("--no-js").arg(dir.path()));
    assert!(
        linted.code <= 1,
        "the migrated config must load: {}{}",
        linted.stdout,
        linted.stderr
    );
    assert!(!linted.stderr.contains("error:"), "{}", linted.stderr);
}

fn version_mismatches(report: &serde_json::Value) -> Vec<String> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["rule"] == "version-mismatch")
        .map(|diagnostic| diagnostic["location"]["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_sherif_migration_does_not_compare_dependents_with_the_version_of_a_workspace_package() {
    let dir = project(
        r#"{ "name": "demo", "private": true, "packageManager": "npm@10.9.0", "workspaces": ["packages/*"],
             "scripts": { "check": "sherif" } }"#,
    );
    for (name, manifest) in [
        (
            "a",
            r#"{ "name": "@x/a", "version": "1.0.0", "dependencies": { "@x/b": "1.0.0" } }"#,
        ),
        ("b", r#"{ "name": "@x/b", "version": "1.0.1" }"#),
    ] {
        let package = dir.path().join("packages").join(name);
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("package.json"), manifest).unwrap();
    }
    let unmigrated = run(skif()
        .args(["--no-js", "--no-config", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(
        version_mismatches(&unmigrated.json()),
        ["packages/a/package.json"],
        "skif's default scope compares @x/b@1.0.0 with the workspace version: {}",
        unmigrated.stdout
    );

    let written = run(&mut migrate(dir.path()));
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    let linted = run(skif().args(["--no-js", "--format", "json"]).arg(dir.path()));
    assert_eq!(
        version_mismatches(&linted.json()),
        Vec::<String>::new(),
        "sherif never compares a dependency with a package's own version: {}",
        linted.stdout
    );
}

#[test]
fn merge_keeps_the_comments_of_the_existing_file() {
    let dir = project(SHERIF_PACKAGE);
    fs::write(
        dir.path().join("skif.toml"),
        "# mine\n[workspace]\nignore-packages = [\"legacy\"]\n",
    )
    .unwrap();
    let merged = run(migrate(dir.path()).args(["--merge", "--profile", "full"]));
    assert_eq!(merged.code, 0, "{}{}", merged.stdout, merged.stderr);
    let text = fs::read_to_string(dir.path().join("skif.toml")).unwrap();
    assert!(
        text.starts_with("# mine\n[workspace]\nignore-packages = [\"legacy\"]\n"),
        "{text}"
    );
    assert!(
        text.contains("[policy]\ndependency-types = [\"prod\", \"dev\"]\nprefer = \"lowest\""),
        "{text}"
    );
}

#[test]
fn force_replaces_the_existing_file() {
    let dir = project(SHERIF_PACKAGE);
    fs::write(dir.path().join("skif.toml"), "[lint]\nfail-on-warnings = false\n").unwrap();
    let forced = run(migrate(dir.path()).arg("--force"));
    assert_eq!(forced.code, 0, "{}{}", forced.stdout, forced.stderr);
    let text = fs::read_to_string(dir.path().join("skif.toml")).unwrap();
    assert!(text.contains("fail-on-warnings = true"), "{text}");
}

#[test]
fn json_output_is_one_document() {
    let dir = project(SHERIF_PACKAGE);
    let result = run(migrate(dir.path()).args(["--dry-run", "--format", "json", "--profile", "full"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let document = result.json();
    assert_eq!(document["version"], 1);
    assert_eq!(document["profile"], "full");
    assert_eq!(document["written"], serde_json::Value::Null);
    assert!(document["toml"].as_str().unwrap().contains("[lint]"));
    assert!(document["translated"].as_array().unwrap().len() >= 3);
    assert_eq!(document["tools"], serde_json::json!(["sherif"]));
}

#[test]
fn a_project_without_any_tool_is_a_usage_error() {
    let dir = project(r#"{ "name": "demo" }"#);
    let result = run(&mut migrate(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(result.stderr.contains("nothing to migrate"), "{}", result.stderr);
    let named = run(migrate(dir.path()).args(["--from", "knip", "--dry-run", "--profile", "full"]));
    assert_eq!(
        named.code, 0,
        "naming the tool migrates its defaults: {}{}",
        named.stdout, named.stderr
    );
}

#[test]
fn knip_files_are_discovered_and_strict_fails_on_what_needs_a_human() {
    let dir = project(r#"{ "name": "demo" }"#);
    fs::write(
        dir.path().join("knip.json"),
        r#"{ "entry": ["src/index.ts!"], "ignoreDependencies": ["left-pad"], "madeUp": true }"#,
    )
    .unwrap();
    let lenient = run(migrate(dir.path()).args(["--dry-run", "--profile", "full"]));
    assert_eq!(lenient.code, 0, "{}{}", lenient.stdout, lenient.stderr);
    assert!(
        lenient.stdout.contains("entry = [\"src/index.ts!\"]"),
        "{}",
        lenient.stdout
    );
    assert!(lenient.stderr.contains("madeUp"), "{}", lenient.stderr);
    let strict = run(migrate(dir.path()).args(["--dry-run", "--strict"]));
    assert_eq!(strict.code, 1, "{}{}", strict.stdout, strict.stderr);
}

#[test]
fn source_needs_exactly_one_tool() {
    let dir = project(SHERIF_PACKAGE);
    let result = run(migrate(dir.path()).args(["--source", "knip.json", "--from", "sherif,knip"]));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(result.stderr.contains("--source"), "{}", result.stderr);
}

#[test]
fn an_unreadable_configuration_names_the_file() {
    let dir = project(r#"{ "name": "demo" }"#);
    fs::write(dir.path().join(".syncpackrc.yaml"), "versionGroups: [unclosed\n  - a\n").unwrap();
    let result = run(migrate(dir.path()).args(["--from", "syncpack", "--dry-run"]));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(result.stderr.contains(".syncpackrc.yaml"), "{}", result.stderr);
}

#[test]
fn force_replaces_the_dot_file_that_exists_instead_of_adding_a_second_one() {
    let dir = project(SHERIF_PACKAGE);
    fs::write(dir.path().join(".skif.toml"), "[lint]\nfail-on-warnings = false\n").unwrap();
    let forced = run(migrate(dir.path()).arg("--force"));
    assert_eq!(forced.code, 0, "{}{}", forced.stdout, forced.stderr);
    assert!(
        !dir.path().join("skif.toml").exists(),
        "a second configuration file would be ambiguous"
    );
    let text = fs::read_to_string(dir.path().join(".skif.toml")).unwrap();
    assert!(text.contains("fail-on-warnings = true"), "{text}");
}

fn migrated(dir: &tempfile::TempDir) -> toml::Table {
    fs::read_to_string(dir.path().join("skif.toml"))
        .unwrap()
        .parse()
        .unwrap()
}

fn with_knip(dir: &tempfile::TempDir) {
    fs::write(dir.path().join("knip.json"), r#"{ "entry": ["src/index.ts"] }"#).unwrap();
}

#[test]
fn migrating_one_tool_after_the_other_checks_what_one_command_checks() {
    let one_shot = project(SHERIF_PACKAGE);
    with_knip(&one_shot);
    let result = run(migrate(one_shot.path()).args(["--from", "sherif,knip"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let expected = migrated(&one_shot);

    for steps in [["sherif", "knip"], ["knip", "sherif"]] {
        let stepwise = project(SHERIF_PACKAGE);
        with_knip(&stepwise);
        let first = run(migrate(stepwise.path()).args(["--from", steps[0]]));
        assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
        let second = run(migrate(stepwise.path()).args(["--from", steps[1], "--merge"]));
        assert_eq!(second.code, 0, "{}{}", second.stdout, second.stderr);
        let actual = migrated(&stepwise);
        assert_eq!(actual["rules"], expected["rules"], "{steps:?}");
        assert_eq!(
            actual["js"].get("enabled"),
            expected["js"].get("enabled"),
            "{steps:?}: the JavaScript analysis stays on"
        );
        assert_ne!(actual["rules"]["unused-files"].as_str(), Some("off"), "{steps:?}");
        assert_ne!(
            actual["rules"].get("version-mismatch").and_then(toml::Value::as_str),
            Some("off")
        );
    }
}

#[test]
fn the_exact_profile_turns_off_every_default_rule_the_tool_did_not_run() {
    let rules = run(skif().args(["rules", "--format", "json"])).json();
    let default_on: BTreeSet<String> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|rule| !rule["default"].is_null())
        .map(|rule| rule["id"].as_str().unwrap().to_owned())
        .collect();
    let dir = project(SHERIF_PACKAGE);
    let result = run(migrate(dir.path()).args(["--dry-run", "--format", "json"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let extra: BTreeSet<String> = result.json()["extra-checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    let sherif: BTreeSet<String> = [
        "empty-dependencies",
        "version-mismatch",
        "unmatched-workspace-patterns",
        "missing-package-manifest",
        "root-package-dependencies",
        "root-package-manager-field",
        "root-package-private-field",
        "types-in-dependencies",
        "unsorted-dependencies",
        "unsynced-dependency-family",
    ]
    .map(str::to_owned)
    .into();
    assert!(extra.is_subset(&default_on), "{extra:?}");
    let left: BTreeSet<String> = default_on.difference(&extra).cloned().collect();
    assert!(left.is_subset(&sherif), "{left:?}");
    assert!(extra.contains("unused-files"));
}

#[test]
fn config_hints_as_errors_are_written_as_warn_unless_strict_hints_is_given() {
    let dir = project(r#"{ "name": "demo", "scripts": { "knip": "knip --treat-config-hints-as-errors" } }"#);
    let plain = run(migrate(dir.path()).args(["--from", "knip", "--dry-run"]));
    assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
    assert!(plain.stdout.contains("stale-js-config = \"warn\""), "{}", plain.stdout);
    let strict = run(migrate(dir.path()).args(["--from", "knip", "--dry-run", "--strict-hints"]));
    assert_eq!(strict.code, 0, "{}{}", strict.stdout, strict.stderr);
    assert!(
        strict.stdout.contains("stale-js-config = \"error\""),
        "{}",
        strict.stdout
    );
}

/// The `stale-js-config` messages of a run over `dir`.
fn stale_config(dir: &Path) -> Vec<String> {
    let linted = run(skif().args(["--format", "json"]).arg(dir));
    assert!(linted.code <= 1, "{}{}", linted.stdout, linted.stderr);
    linted.json()["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["rule"] == "stale-js-config")
        .map(|diagnostic| diagnostic["message"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn stylesheets_in_a_migrated_knip_project_are_not_reported_stale() {
    let migrated_run = |extensions: &str| {
        let dir = project(r#"{ "name": "demo", "private": true }"#);
        // `treatConfigHintsAsErrors` makes the migration write `stale-js-config = "warn"`.
        let knip = format!(
            "{{ \"entry\": [\"src/index.ts!\"], \"project\": [\"src/**/*.{{{extensions}}}!\"], \
             \"treatConfigHintsAsErrors\": true }}"
        );
        fs::write(dir.path().join("knip.json"), knip).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/index.ts"), "import \"./app.css\";\n").unwrap();
        fs::write(dir.path().join("src/app.css"), "a { color: red; }\n").unwrap();
        fs::write(dir.path().join("src/data.foo"), "x\n").unwrap();
        let result = run(migrate(dir.path()).args(["--from", "knip"]));
        assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
        let written = fs::read_to_string(dir.path().join("skif.toml")).unwrap();
        assert!(
            written.contains(&format!("project = [\"src/**/*.{{{extensions}}}!\"]")),
            "the knip glob is kept verbatim: {written}"
        );
        assert!(written.contains("stale-js-config = \"warn\""), "{written}");
        stale_config(dir.path())
    };
    assert_eq!(migrated_run("ts,css"), Vec::<String>::new());
    assert_eq!(
        migrated_run("ts,css,foo"),
        ["`.foo` files selected by `js.project` are not analysed"]
    );
}
