//! End-to-end tests of layered configuration: an organisation file, a preset it extends, a pnpm project with a
//! project file and a package file, a standalone package that opts out, and scoped overrides.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use support::{fixture, run, skif};

fn json(path: &Path, extra: &[&str]) -> (i32, Value) {
    let result = run(skif().args(["--format", "json"]).args(extra).arg(path));
    assert!(
        result.code == 0 || result.code == 1,
        "{}{}",
        result.stdout,
        result.stderr
    );
    (result.code, result.json())
}

/// The severity of every `empty-dependencies` diagnostic by file.
fn levels(report: &Value) -> BTreeMap<String, String> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["rule"] == "empty-dependencies")
        .map(|diagnostic| {
            (
                diagnostic["location"]["path"].as_str().unwrap().to_owned(),
                diagnostic["severity"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn expected(levels: &[(&str, &str)]) -> BTreeMap<String, String> {
    levels
        .iter()
        .map(|(path, level)| ((*path).to_owned(), (*level).to_owned()))
        .collect()
}

#[test]
fn every_layer_decides_the_level_of_its_own_packages() {
    let dir = fixture("inheritance");
    let (code, report) = json(dir.path(), &["-r"]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(
        levels(&report),
        expected(&[
            // the preset the organisation file extends
            ("web/packages/plain/package.json", "warning"),
            // an override of the organisation file, selected by directory
            ("web/packages/gen/package.json", "info"),
            // an override of the project file, selected by name
            ("web/packages/app/package.json", "error"),
            // `root = true` opts out of the organisation file and its preset, so the rule's own default applies
            ("tools/standalone/package.json", "error"),
            // the package file of `legacy-ui` turns the rule off for that package
        ])
    );
}

#[test]
fn the_package_file_only_silences_its_own_package() {
    let dir = fixture("inheritance");
    let (_, report) = json(dir.path(), &["-r"]);
    let paths: Vec<_> = levels(&report).into_keys().collect();
    assert!(paths.iter().all(|path| !path.contains("legacy-ui")), "{paths:?}");
    assert!(paths.iter().any(|path| path.contains("plain")), "{paths:?}");
}

#[test]
fn a_project_run_on_its_own_sees_the_same_levels() {
    let dir = fixture("inheritance");
    fs::create_dir(dir.path().join(".git")).unwrap();
    let (_, project) = json(&dir.path().join("web"), &[]);
    assert_eq!(
        levels(&project),
        expected(&[
            ("packages/plain/package.json", "warning"),
            ("packages/gen/package.json", "info"),
            ("packages/app/package.json", "error"),
        ])
    );
}

#[test]
fn a_command_line_level_beats_every_override() {
    let dir = fixture("inheritance");
    let (_, report) = json(dir.path(), &["-r", "--rule", "empty-dependencies=info"]);
    let found = levels(&report);
    assert!(!found.is_empty());
    assert!(found.values().all(|level| level == "info"), "{found:?}");
}

#[test]
fn a_package_file_with_a_root_scoped_key_fails_the_run() {
    let dir = fixture("inheritance");
    fs::write(
        dir.path().join("web/packages/legacy-ui/skif.toml"),
        "[workspace]\nignore-packages = [\"x\"]\n",
    )
    .unwrap();
    let result = run(skif().arg(dir.path().join("web")));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(result.stderr.contains("workspace"), "{}", result.stderr);
    assert!(result.stderr.contains("legacy-ui"), "{}", result.stderr);
}

#[test]
fn an_override_that_selects_nothing_leaves_the_output_byte_identical() {
    let dir = fixture("multi/nested");
    let before = run(skif().args(["--format", "json", "-r"]).arg(dir.path()));
    let config = dir.path().join("skif.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        "\n[[overrides]]\npackages = [\"nobody\"]\nmax-level = \"info\"\n\n[overrides.rules]\nempty-dependencies = \"off\"\n",
    );
    fs::write(&config, text).unwrap();
    let after = run(skif().args(["--format", "json", "-r"]).arg(dir.path()));
    assert_eq!(after.code, before.code);
    assert_eq!(after.stdout, before.stdout);
}

#[test]
fn an_override_pattern_that_leaves_its_anchor_is_warned_about() {
    let dir = fixture("mismatch");
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(
        dir.path().join("config").join("shared.toml"),
        "[[overrides]]\npackages = [\"../packages/*\"]\nrules = { version-mismatch = \"off\" }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("skif.toml"),
        "extends = [\"config/shared.toml\"]\n\n[js]\nenabled = false\n",
    )
    .unwrap();
    let result = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(result.code, 1, "the pattern matches nothing: {}", result.stdout);
    assert!(
        result
            .stderr
            .contains("shared.toml: the [[overrides]] pattern `../packages/*` leaves the directory")
            && result.stderr.contains("$ROOT/"),
        "{}",
        result.stderr
    );
    assert_eq!(result.stderr.matches("../packages/*").count(), 1, "{}", result.stderr);
}
