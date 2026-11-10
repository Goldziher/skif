//! `skif baseline`: writing, pruning and ratcheting a suppression baseline.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use support::{Run, fixture, run, skif};

const BASELINE: &str = "skif.baseline.toml";

fn baseline(dir: &Path, command: &str, extra: &[&str]) -> Run {
    run(skif()
        .args(["baseline", command])
        .arg(dir)
        .args(extra)
        .args(["--format", "json"]))
}

fn lint(dir: &Path) -> Run {
    run(skif().args(["--format", "json"]).arg(dir))
}

fn set_is_number(dir: &Path, member: &str, version: &str) {
    let path = dir.join("packages").join(member).join("package.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let manifest = format!(
        "{{\n  \"name\": \"@cli/{member}\",\n  \"version\": \"0.0.0\",\n  \"dependencies\": {{\n    \"is-number\": \"{version}\"\n  }}\n}}\n"
    );
    fs::write(path, manifest).unwrap();
}

fn entry(member: &str, version: &str) -> Value {
    json!({
        "rules": ["version-mismatch"],
        "packages": [format!("@cli/{member}")],
        "dependencies": [format!("is-number@{version}")],
        "reason": "baseline",
    })
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=skif",
            "-c",
            "user.email=skif@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn write_records_the_findings_and_the_lint_run_is_clean_after_it() {
    let dir = fixture("mismatch");
    let before = lint(dir.path());
    assert_eq!(before.code, 1, "{}{}", before.stdout, before.stderr);
    let findings = before.json()["diagnostics"].as_array().unwrap().len();

    let written = baseline(dir.path(), "write", &[]);
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    let document = written.json();
    assert_eq!(document["command"], "write");
    assert_eq!(document["file"], BASELINE);
    assert_eq!(document["added"], json!([entry("b", "6.0.0")]));
    assert_eq!(document["added"].as_array().unwrap().len(), findings);
    assert_eq!(document["entries"], 1);
    assert_eq!(document["extended"], "skif.toml");
    assert_eq!(document["written"], true);

    let text = fs::read_to_string(dir.path().join(BASELINE)).unwrap();
    assert!(
        text.contains(
            "[[ignore]]\nrules = [\"version-mismatch\"]\npackages = [\"@cli/b\"]\ndependencies = [\"is-number@6.0.0\"]\nreason = \"baseline\"\n"
        ),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("skif.toml")).unwrap(),
        "extends = [\"skif.baseline.toml\"]\n\n[js]\nenabled = false\n"
    );

    let after = lint(dir.path());
    assert_eq!(after.code, 0, "{}{}", after.stdout, after.stderr);
    assert_eq!(after.json()["diagnostics"], json!([]));
}

#[test]
fn writing_again_changes_nothing() {
    let dir = fixture("mismatch");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    let baseline_text = fs::read_to_string(dir.path().join(BASELINE)).unwrap();
    let config_text = fs::read_to_string(dir.path().join("skif.toml")).unwrap();

    let again = baseline(dir.path(), "write", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    let document = again.json();
    assert_eq!(document["added"], json!([]));
    assert_eq!(document["entries"], 1);
    assert_eq!(document["extended"], Value::Null);
    assert_eq!(document["written"], false);
    assert_eq!(fs::read_to_string(dir.path().join(BASELINE)).unwrap(), baseline_text);
    assert_eq!(fs::read_to_string(dir.path().join("skif.toml")).unwrap(), config_text);
}

#[test]
fn write_merges_new_findings_and_keeps_edited_reasons() {
    let dir = fixture("mismatch");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    let path = dir.path().join(BASELINE);
    let edited = fs::read_to_string(&path)
        .unwrap()
        .replace("reason = \"baseline\"", "reason = \"waiting for the b release\"");
    fs::write(&path, edited).unwrap();
    set_is_number(dir.path(), "c", "5.0.0");

    let merged = baseline(dir.path(), "write", &["--reason", "adopted"]);
    assert_eq!(merged.code, 0, "{}{}", merged.stdout, merged.stderr);
    let mut added = entry("c", "5.0.0");
    added["reason"] = json!("adopted");
    assert_eq!(merged.json()["added"], json!([added]));
    assert_eq!(merged.json()["entries"], 2);
    let text = fs::read_to_string(&path).unwrap();
    let b = text.find("\"@cli/b\"").unwrap();
    let c = text.find("\"@cli/c\"").unwrap();
    assert!(b < c, "entries are sorted: {text}");
    assert!(text.contains("reason = \"waiting for the b release\""), "{text}");
    assert!(text.contains("reason = \"adopted\""), "{text}");
    assert_eq!(lint(dir.path()).code, 0);
}

#[test]
fn a_baselined_version_still_fails_when_a_third_version_appears() {
    let dir = fixture("mismatch");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    set_is_number(dir.path(), "b", "5.0.0");

    let moved = lint(dir.path());
    assert_eq!(moved.code, 1, "{}{}", moved.stdout, moved.stderr);
    let diagnostics = moved.json()["diagnostics"].clone();
    let reported: Vec<(&str, &str, &str)> = diagnostics
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| {
            (
                diagnostic["rule"].as_str().unwrap(),
                diagnostic["package"].as_str().unwrap(),
                diagnostic["dependency"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(reported, [("version-mismatch", "@cli/b", "is-number")]);
    assert!(
        diagnostics[0]["message"].as_str().unwrap().contains("is-number@5.0.0"),
        "{diagnostics}"
    );
}

#[test]
fn prune_removes_the_entries_a_fix_left_unmatched() {
    let dir = fixture("mismatch");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    set_is_number(dir.path(), "b", "7.0.0");

    let pruned = baseline(dir.path(), "prune", &[]);
    assert_eq!(pruned.code, 0, "{}{}", pruned.stdout, pruned.stderr);
    let document = pruned.json();
    assert_eq!(document["command"], "prune");
    assert_eq!(document["removed"], json!([entry("b", "6.0.0")]));
    assert_eq!(document["entries"], 0);
    assert_eq!(document["written"], true);
    let text = fs::read_to_string(dir.path().join(BASELINE)).unwrap();
    assert!(!text.contains("[[ignore]]"), "{text}");
    assert_eq!(lint(dir.path()).code, 0, "the extended file still resolves");

    let again = baseline(dir.path(), "prune", &[]);
    assert_eq!(again.code, 0);
    assert_eq!(again.json()["removed"], json!([]));
    assert_eq!(again.json()["written"], false);
}

#[test]
fn prune_keeps_the_entries_that_still_match() {
    let dir = fixture("mismatch");
    set_is_number(dir.path(), "c", "5.0.0");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    set_is_number(dir.path(), "c", "7.0.0");

    let pruned = baseline(dir.path(), "prune", &[]);
    assert_eq!(pruned.code, 0, "{}{}", pruned.stdout, pruned.stderr);
    assert_eq!(pruned.json()["removed"], json!([entry("c", "5.0.0")]));
    assert_eq!(pruned.json()["entries"], 1);
    assert_eq!(lint(dir.path()).code, 0);
}

#[test]
fn prune_without_a_baseline_file_cannot_run() {
    let dir = fixture("mismatch");
    let pruned = baseline(dir.path(), "prune", &[]);
    assert_eq!(pruned.code, 2, "{}{}", pruned.stdout, pruned.stderr);
    assert_eq!(pruned.json()["error"]["kind"], "config");
}

#[test]
fn check_fails_when_the_baseline_grew_and_passes_when_it_shrank() {
    let dir = fixture("mismatch");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    let old = tempfile::NamedTempFile::new().unwrap();
    fs::copy(dir.path().join(BASELINE), old.path()).unwrap();
    let base = old.path().to_str().unwrap();

    let unchanged = baseline(dir.path(), "check", &["--base-file", base]);
    assert_eq!(unchanged.code, 0, "{}{}", unchanged.stdout, unchanged.stderr);
    assert_eq!(unchanged.json()["added"], json!([]));

    set_is_number(dir.path(), "c", "5.0.0");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    let grown = baseline(dir.path(), "check", &["--base-file", base]);
    assert_eq!(grown.code, 1, "{}{}", grown.stdout, grown.stderr);
    assert_eq!(grown.json()["command"], "check");
    assert_eq!(grown.json()["added"], json!([entry("c", "5.0.0")]));
    assert_eq!(grown.json()["removed"], json!([]));

    set_is_number(dir.path(), "b", "7.0.0");
    set_is_number(dir.path(), "c", "7.0.0");
    assert_eq!(baseline(dir.path(), "prune", &[]).code, 0);
    let shrunk = baseline(dir.path(), "check", &["--base-file", base]);
    assert_eq!(shrunk.code, 0, "{}{}", shrunk.stdout, shrunk.stderr);
    assert_eq!(shrunk.json()["added"], json!([]));
    assert_eq!(shrunk.json()["removed"], json!([entry("b", "6.0.0")]));
}

#[test]
fn check_ignores_edited_reasons() {
    let dir = fixture("mismatch");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    let old = tempfile::NamedTempFile::new().unwrap();
    fs::copy(dir.path().join(BASELINE), old.path()).unwrap();
    let path = dir.path().join(BASELINE);
    let edited = fs::read_to_string(&path)
        .unwrap()
        .replace("reason = \"baseline\"", "reason = \"tracked elsewhere\"");
    fs::write(&path, edited).unwrap();

    let checked = baseline(dir.path(), "check", &["--base-file", old.path().to_str().unwrap()]);
    assert_eq!(checked.code, 0, "{}{}", checked.stdout, checked.stderr);
}

#[test]
fn check_against_a_git_revision() {
    let dir = fixture("mismatch");
    git(dir.path(), &["init", "--quiet"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "--quiet", "-m", "before the baseline"]);

    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    let new_file = baseline(dir.path(), "check", &["--base", "HEAD"]);
    assert_eq!(
        new_file.code, 1,
        "a baseline that did not exist at the revision is empty there: {}{}",
        new_file.stdout, new_file.stderr
    );
    assert_eq!(new_file.json()["base"], "HEAD");
    assert_eq!(new_file.json()["added"], json!([entry("b", "6.0.0")]));

    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "--quiet", "-m", "the baseline"]);
    let committed = baseline(dir.path(), "check", &["--base", "HEAD"]);
    assert_eq!(committed.code, 0, "{}{}", committed.stdout, committed.stderr);

    set_is_number(dir.path(), "c", "5.0.0");
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    let grown = baseline(dir.path(), "check", &["--base", "HEAD"]);
    assert_eq!(grown.code, 1, "{}{}", grown.stdout, grown.stderr);
    assert_eq!(grown.json()["added"], json!([entry("c", "5.0.0")]));

    let unknown = baseline(dir.path(), "check", &["--base", "no-such-revision"]);
    assert_eq!(unknown.code, 2, "{}{}", unknown.stdout, unknown.stderr);
    assert_eq!(unknown.json()["error"]["kind"], "usage");
}

#[test]
fn check_needs_a_base() {
    let dir = fixture("mismatch");
    let missing = run(skif().args(["baseline", "check"]).arg(dir.path()));
    assert_eq!(missing.code, 2, "{}{}", missing.stdout, missing.stderr);
}

#[test]
fn a_custom_file_is_extended_by_its_relative_path() {
    let dir = fixture("mismatch");
    let file = dir.path().join("config").join("debt.toml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let written = baseline(dir.path(), "write", &["--file", file.to_str().unwrap()]);
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    assert_eq!(written.json()["file"], "config/debt.toml");
    let config = fs::read_to_string(dir.path().join("skif.toml")).unwrap();
    assert!(config.starts_with("extends = [\"config/debt.toml\"]\n"), "{config}");
    assert_eq!(lint(dir.path()).code, 0);
}

#[test]
fn an_existing_extends_list_gains_the_baseline_and_keeps_its_formatting() {
    let dir = fixture("mismatch");
    fs::write(dir.path().join("shared.toml"), "[lint]\nfail-on-warnings = false\n").unwrap();
    fs::write(
        dir.path().join("skif.toml"),
        "#:schema ./schema.json\n# shared settings first\nextends = [\"shared.toml\"]\n\n[js]\nenabled = false # no JS here\n",
    )
    .unwrap();
    assert_eq!(baseline(dir.path(), "write", &[]).code, 0);
    assert_eq!(
        fs::read_to_string(dir.path().join("skif.toml")).unwrap(),
        "#:schema ./schema.json\n# shared settings first\nextends = [\"shared.toml\", \"skif.baseline.toml\"]\n\n[js]\nenabled = false # no JS here\n"
    );
    assert_eq!(lint(dir.path()).code, 0);
}

#[test]
fn a_baseline_file_with_other_settings_is_refused() {
    let dir = fixture("mismatch");
    fs::write(dir.path().join(BASELINE), "[lint]\nfail-on-warnings = true\n").unwrap();
    let written = baseline(dir.path(), "write", &[]);
    assert_eq!(written.code, 2, "{}{}", written.stdout, written.stderr);
    assert_eq!(written.json()["error"]["kind"], "config");
    assert!(
        written.json()["error"]["message"].as_str().unwrap().contains("`lint`"),
        "{}",
        written.stdout
    );
}

#[test]
fn human_output_lists_the_entries() {
    let dir = fixture("mismatch");
    let written = run(skif().args(["baseline", "write"]).arg(dir.path()));
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    assert_eq!(
        written.stdout,
        "Recorded 1 new entry in skif.baseline.toml (1 in total)\n  + version-mismatch: package @cli/b, dependency is-number@6.0.0\nAdded skif.baseline.toml to `extends` in skif.toml\n"
    );
}

/// Removes the name of the fixture's root package and gives it `is-number@5.0.0`, a third version.
fn unname_root(dir: &Path) {
    fs::write(
        dir.join("package.json"),
        "{\n  \"version\": \"0.0.0\",\n  \"private\": true,\n  \"packageManager\": \"npm@11.0.0\",\n  \"workspaces\": [\n    \"packages/*\"\n  ],\n  \"devDependencies\": {\n    \"is-number\": \"5.0.0\"\n  }\n}\n",
    )
    .unwrap();
}

fn version_findings(document: &Value, key: &str) -> Vec<Value> {
    document[key]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["rule"] == "version-mismatch")
        .cloned()
        .collect()
}

#[test]
fn a_finding_in_an_unnamed_root_is_not_recorded() {
    let dir = fixture("mismatch");
    unname_root(dir.path());

    let written = baseline(dir.path(), "write", &[]);
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    let document = written.json();
    assert_eq!(
        document["added"],
        json!([entry("b", "6.0.0")]),
        "no entry without a package for the root's is-number@5.0.0"
    );
    let unrecorded = version_findings(&document, "unrecorded");
    assert_eq!(unrecorded.len(), 1, "{document}");
    assert_eq!(unrecorded[0]["dependency"], "is-number");
    assert_eq!(unrecorded[0]["cause"], "unnamed-root");

    let after = lint(dir.path());
    assert_eq!(after.code, 1, "the root's finding is still reported: {}", after.stdout);

    set_is_number(dir.path(), "c", "5.0.0");
    let later = lint(dir.path());
    let packages: Vec<Value> = version_findings(&later.json(), "diagnostics")
        .iter()
        .map(|diagnostic| diagnostic["package"].clone())
        .collect();
    assert!(
        packages.contains(&json!("@cli/c")),
        "a package that adds the root's version is not silenced: {packages:?}"
    );
}

#[test]
fn human_output_says_why_an_unnamed_root_finding_is_not_recorded() {
    let dir = fixture("mismatch");
    unname_root(dir.path());
    let written = run(skif().args(["baseline", "write"]).arg(dir.path()));
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    assert!(
        written.stderr.contains("warning: not recorded: version-mismatch (")
            && written.stderr.contains("the root package has no name of its own"),
        "{}",
        written.stderr
    );
}

#[test]
fn packages_sharing_a_name_are_recorded_by_directory() {
    let dir = fixture("mismatch");
    let path = dir.path().join("packages").join("c").join("package.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "{\n  \"name\": \"@cli/b\",\n  \"version\": \"0.0.0\",\n  \"dependencies\": {\n    \"is-number\": \"5.0.0\"\n  }\n}\n",
    )
    .unwrap();

    let written = baseline(dir.path(), "write", &[]);
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    let by_dir = |dir: &str, version: &str| {
        json!({
            "rules": ["version-mismatch"],
            "packages": [dir],
            "dependencies": [format!("is-number@{version}")],
            "reason": "baseline",
        })
    };
    assert_eq!(
        written.json()["added"],
        json!([by_dir("packages/b", "6.0.0"), by_dir("packages/c", "5.0.0")])
    );
    assert_eq!(lint(dir.path()).code, 0);

    fs::write(
        dir.path().join("packages").join("c").join("package.json"),
        "{\n  \"name\": \"@cli/b\",\n  \"version\": \"0.0.0\",\n  \"dependencies\": {\n    \"is-number\": \"6.0.0\"\n  }\n}\n",
    )
    .unwrap();
    let moved = lint(dir.path());
    assert_eq!(
        moved.code, 1,
        "the entry of packages/b does not cover packages/c: {}",
        moved.stdout
    );
}

#[test]
fn a_relative_file_resolves_from_the_working_directory() {
    let dir = fixture("mismatch");
    let member = dir.path().join("packages").join("a");
    let written =
        run(skif()
            .current_dir(&member)
            .args(["baseline", "write", "--file", "../../debt.toml", "--format", "json"]));
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    assert_eq!(written.json()["file"], "debt.toml");
    assert_eq!(written.json()["extended"], "skif.toml");
    assert!(dir.path().join("debt.toml").is_file());
    assert!(!dir.path().join(BASELINE).exists());
    let config = fs::read_to_string(dir.path().join("skif.toml")).unwrap();
    assert!(config.starts_with("extends = [\"debt.toml\"]\n"), "{config}");

    let again =
        run(skif()
            .current_dir(dir.path())
            .args(["baseline", "write", "--file", "debt.toml", "--format", "json"]));
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(again.json()["extended"], Value::Null, "already extended");
    assert_eq!(again.json()["written"], false);
    assert_eq!(fs::read_to_string(dir.path().join("skif.toml")).unwrap(), config);
}

#[test]
fn a_file_argument_that_names_no_file_is_a_usage_error() {
    let dir = fixture("mismatch");
    let written = baseline(dir.path(), "write", &["--file", ".."]);
    assert_eq!(written.code, 2, "{}{}", written.stdout, written.stderr);
    assert_eq!(written.json()["error"]["kind"], "usage");
}

#[test]
fn check_ignores_the_order_of_an_entry_s_patterns() {
    let dir = fixture("mismatch");
    let base = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        base.path(),
        "[[ignore]]\nrules = [\"version-mismatch\", \"banned-dependency\"]\npackages = [\"@cli/a\", \"@cli/b\"]\n",
    )
    .unwrap();
    fs::write(
        dir.path().join(BASELINE),
        "[[ignore]]\nrules = [\"banned-dependency\", \"version-mismatch\"]\npackages = [\"@cli/b\", \"@cli/a\"]\n",
    )
    .unwrap();
    let checked = baseline(dir.path(), "check", &["--base-file", base.path().to_str().unwrap()]);
    assert_eq!(checked.code, 0, "{}{}", checked.stdout, checked.stderr);
    assert_eq!(checked.json()["added"], json!([]));
    assert_eq!(checked.json()["removed"], json!([]));
}

/// Makes `@cli/a` private with `@types/node` at `version` in its `dependencies`, a `types-in-dependencies` finding.
fn set_types_node(dir: &Path, version: &str) {
    fs::write(
        dir.join("packages").join("a").join("package.json"),
        format!(
            "{{\n  \"name\": \"@cli/a\",\n  \"version\": \"0.0.0\",\n  \"private\": true,\n  \"dependencies\": {{\n    \"@types/node\": \"{version}\",\n    \"is-number\": \"7.0.0\"\n  }}\n}}\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_finding_that_is_not_about_the_version_survives_a_bump() {
    let dir = fixture("mismatch");
    set_types_node(dir.path(), "^22.0.0");
    let before = lint(dir.path());
    let rules: Vec<Value> = before.json()["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["rule"].clone())
        .collect();
    assert!(rules.contains(&json!("types-in-dependencies")), "{}", before.stdout);

    let written = baseline(dir.path(), "write", &[]);
    assert_eq!(written.code, 0, "{}{}", written.stdout, written.stderr);
    assert_eq!(
        written.json()["added"],
        json!([
            {
                "rules": ["types-in-dependencies"],
                "packages": ["@cli/a"],
                "dependencies": ["@types/node"],
                "reason": "baseline",
            },
            entry("b", "6.0.0"),
        ]),
        "the version rule keeps its specifier, the other one records the name alone"
    );

    set_types_node(dir.path(), "^22.1.0");
    let bumped = lint(dir.path());
    assert_eq!(bumped.code, 0, "{}{}", bumped.stdout, bumped.stderr);
    let again = baseline(dir.path(), "write", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(again.json()["added"], json!([]));
    assert_eq!(again.json()["written"], false);
}
