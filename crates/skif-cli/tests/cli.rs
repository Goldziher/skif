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
fn require_ignore_reasons_fails_only_on_entries_of_a_file_without_one() {
    let dir = fixture("dirty");
    let toml = dir.path().join("skif.toml");
    let require = |extra: &[&str]| {
        run(skif()
            .args(["--require-ignore-reasons", "--format", "json"])
            .args(extra)
            .arg(dir.path()))
    };

    std::fs::write(&toml, "[[ignore]]\nrules = [\"empty-dependencies\"]\n").unwrap();
    let missing = require(&[]);
    assert_eq!(missing.code, 2, "{}{}", missing.stdout, missing.stderr);
    assert!(
        format!("{}{}", missing.stdout, missing.stderr).contains("ignore[0]: no `reason`"),
        "{}{}",
        missing.stdout,
        missing.stderr
    );
    let without_flag = run(skif().arg(dir.path()));
    assert_ne!(without_flag.code, 2, "{}{}", without_flag.stdout, without_flag.stderr);

    std::fs::write(
        &toml,
        "[[ignore]]\nrules = [\"empty-dependencies\"]\nreason = \"kept while the package migrates\"\n",
    )
    .unwrap();
    let reasoned = require(&[
        "--ignore-dependency",
        "left-pad",
        "--ignore-rule-for-package",
        "empty-dependencies:@cli/a",
    ]);
    assert_ne!(reasoned.code, 2, "{}{}", reasoned.stdout, reasoned.stderr);
}

#[test]
fn require_ignore_reasons_covers_the_js_ignore_lists_and_the_lint_key() {
    let dir = fixture("dirty");
    let toml = dir.path().join("skif.toml");
    let output = |result: &support::Run| format!("{}{}", result.stdout, result.stderr);
    let lint = |extra: &[&str]| run(skif().args(extra).arg(dir.path()));

    std::fs::write(
        &toml,
        "[js.workspaces.\"packages/app\"]\nignore-dependencies = [{ name = \"left-pad\", reason = \"loaded by the bundler\" }, \"some-loader\"]\n",
    )
    .unwrap();
    let flagged = lint(&["--require-ignore-reasons"]);
    assert_eq!(flagged.code, 2, "{}", output(&flagged));
    assert!(
        output(&flagged).contains(
            "js.workspaces.\"packages/app\".ignore-dependencies[1] `some-loader`: no `reason`, which --require-ignore-reasons requires"
        ),
        "{}",
        output(&flagged)
    );
    let unflagged = lint(&[]);
    assert_ne!(unflagged.code, 2, "{}", output(&unflagged));

    std::fs::write(
        &toml,
        "[lint]\nrequire-ignore-reasons = true\n[js]\nignore-binaries = [\"docker\"]\n",
    )
    .unwrap();
    let by_key = lint(&[]);
    assert_eq!(by_key.code, 2, "{}", output(&by_key));
    assert!(
        output(&by_key)
            .contains("js.ignore-binaries[0] `docker`: no `reason`, which [lint] require-ignore-reasons requires"),
        "{}",
        output(&by_key)
    );

    std::fs::write(
        &toml,
        "[lint]\nrequire-ignore-reasons = true\n[js]\nignore-binaries = [{ name = \"docker\", reason = \"installed on the CI image\" }]\n",
    )
    .unwrap();
    let reasoned = lint(&["--require-ignore-reasons"]);
    assert_ne!(reasoned.code, 2, "{}", output(&reasoned));
}

#[test]
fn a_missing_reason_from_an_extend_key_is_named_in_the_merged_list() {
    let dir = fixture("dirty");
    std::fs::write(
        dir.path().join("base.toml"),
        "[js]\nignore-dependencies = [{ name = \"a\", reason = \"r\" }, { name = \"b\", reason = \"r\" }, { name = \"c\", reason = \"r\" }]\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("skif.toml"),
        "extends = [\"base.toml\"]\n[js]\nextend-ignore-dependencies = [\"left-pad\"]\n",
    )
    .unwrap();
    let result = run(skif().arg("--require-ignore-reasons").arg(dir.path()));
    let output = format!("{}{}", result.stdout, result.stderr);
    assert_eq!(result.code, 2, "{output}");
    assert!(
        output.contains(
            "js.ignore-dependencies[3] `left-pad`: no `reason`, which --require-ignore-reasons requires (the position is \
             in the list merged from every config file and its `extend-*` keys)"
        ),
        "{output}"
    );
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
    fs::remove_file(dir.path().join("skif.toml")).unwrap();
    let created = run(skif().arg("init").arg(dir.path()));
    assert_eq!(created.code, 0, "{}{}", created.stdout, created.stderr);
    let target = dir.path().join("skif.toml");
    let starter = fs::read_to_string(&target).unwrap();
    assert!(starter.starts_with("#:schema "), "{starter}");
    insta::assert_snapshot!("init_config", starter);

    let linted = run(skif().arg("--no-js").arg(dir.path()));
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

fn reported(result: &support::Run) -> BTreeSet<String> {
    rule_ids(&result.json())
}

#[test]
fn js_analysis_runs_by_default_and_no_js_turns_it_off() {
    let dir = fixture("js");
    let with_js = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(with_js.code, 1, "{}{}", with_js.stdout, with_js.stderr);
    let found = reported(&with_js);
    for rule in ["unused-dependencies", "unlisted-dependencies"] {
        assert!(found.contains(rule), "{rule} missing from {found:?}");
    }

    let without = run(skif().args(["--no-js", "--format", "json"]).arg(dir.path()));
    assert_eq!(without.code, 0, "{}{}", without.stdout, without.stderr);
    let found = reported(&without);
    for rule in [
        "unused-dependencies",
        "unlisted-dependencies",
        "unused-dev-dependencies",
    ] {
        assert!(!found.contains(rule), "{rule} reported with --no-js: {found:?}");
    }
}

#[test]
fn production_skips_dev_dependencies() {
    let dir = fixture("js");
    let default = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert!(reported(&default).contains("unused-dev-dependencies"));
    let production = run(skif().args(["--production", "--format", "json"]).arg(dir.path()));
    let found = reported(&production);
    assert!(!found.contains("unused-dev-dependencies"), "{found:?}");
    assert!(found.contains("unused-dependencies"), "{found:?}");
}

fn is_number_versions(dir: &Path) -> Vec<String> {
    ["a", "b"]
        .iter()
        .map(|member| {
            let text = fs::read_to_string(dir.join("packages").join(member).join("package.json")).unwrap();
            let manifest: Value = serde_json::from_str(&text).unwrap();
            manifest["dependencies"]["is-number"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn select_decides_which_version_alignment_is_applied() {
    for (select, expected) in [("highest", "7.0.0"), ("lowest", "6.0.0")] {
        let dir = fixture("mismatch");
        let result = run(skif()
            .args(["--fix-unsafe", "--no-install", "--select", select, "--format", "json"])
            .arg(dir.path()));
        assert_eq!(result.code, 0, "{select}\n{}{}", result.stdout, result.stderr);
        assert_eq!(is_number_versions(dir.path()), [expected, expected], "{select}");
    }
}

#[test]
fn color_always_emits_escapes_and_never_does_not() {
    let dir = fixture("dirty");
    let always = run(skif().args(["--color", "always"]).arg(dir.path()));
    assert!(always.stdout.contains('\x1b'), "{}", always.stdout);
    let never = run(skif().args(["--color", "never"]).arg(dir.path()));
    assert!(!never.stdout.contains('\x1b'), "{}", never.stdout);
    let json = run(skif().args(["--color", "always", "--format", "json"]).arg(dir.path()));
    assert!(!json.stdout.contains('\x1b'));
}

#[test]
fn info_findings_alone_exit_zero() {
    let dir = fixture("dirty");
    let result = run(skif()
        .args([
            "--rule",
            "root-package-private-field=off",
            "--rule",
            "empty-dependencies=info",
            "--format",
            "json",
        ])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let summary = &result.json()["summary"];
    assert_eq!(summary["errors"], 0);
    assert_eq!(summary["warnings"], 0);
    assert_ne!(summary["infos"], 0);
}

#[test]
fn fail_on_warnings_conflicts_with_max_warnings() {
    let dir = fixture("dirty");
    let result = run(skif()
        .args(["--fail-on-warnings", "--max-warnings", "3", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 2);
    assert_eq!(result.json()["error"]["kind"], "usage");
}

#[test]
fn sarif_goes_to_stdout_or_to_output() {
    let dir = fixture("dirty");
    let stdout = run(skif().args(["--format", "sarif"]).arg(dir.path()));
    assert_eq!(stdout.code, 1);
    let log = stdout.json();
    assert_eq!(log["version"], "2.1.0");
    assert_ne!(log["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap().len(), 0);
    assert_ne!(log["runs"][0]["results"].as_array().unwrap().len(), 0);

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("skif.sarif");
    let written = run(skif()
        .args(["--format", "sarif", "--output"])
        .arg(&target)
        .arg(dir.path()));
    assert_eq!(written.code, 1);
    assert!(written.stdout.is_empty(), "{}", written.stdout);
    let from_file: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(from_file, log);
    assert!(
        fs::read_dir(out.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".skif-tmp"))
    );
}

#[test]
fn github_format_prints_annotations() {
    let dir = fixture("dirty");
    let result = run(skif().args(["--format", "github"]).arg(dir.path()));
    assert_eq!(result.code, 1);
    assert!(
        result.stdout.lines().any(|line| line.starts_with("::error file=")),
        "{}",
        result.stdout
    );
}

#[test]
fn github_actions_defaults_to_human_plus_annotations() {
    let dir = fixture("dirty");
    let result = run(skif().env("GITHUB_ACTIONS", "true").arg(dir.path()));
    assert_eq!(result.code, 1);
    assert!(result.stdout.contains("Found "), "{}", result.stdout);
    assert!(
        result.stdout.lines().any(|line| line.starts_with("::error file=")),
        "{}",
        result.stdout
    );

    let explicit = run(skif()
        .env("GITHUB_ACTIONS", "true")
        .args(["--format", "json"])
        .arg(dir.path()));
    explicit.json();
}

#[test]
fn fix_through_a_symlink_out_of_the_workspace_is_refused() {
    #[cfg(unix)]
    {
        let dir = fixture("dirty");
        let outside = tempfile::tempdir().unwrap();
        let real = outside.path().join("package.json");
        fs::copy(dir.path().join("packages/a/package.json"), &real).unwrap();
        fs::remove_file(dir.path().join("packages/a/package.json")).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("packages/a/package.json")).unwrap();
        let before = fs::read_to_string(&real).unwrap();
        let root_before = fs::read_to_string(dir.path().join("package.json")).unwrap();

        let result = run(skif()
            .args(["--fix", "--no-install", "--format", "json"])
            .arg(dir.path()));
        assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
        assert_eq!(fs::read_to_string(&real).unwrap(), before);
        assert_eq!(
            fs::read_to_string(dir.path().join("package.json")).unwrap(),
            root_before
        );
    }
}

#[test]
fn pnpm_is_asked_to_update_the_lockfile_after_a_fix() {
    let dir = fixture("dirty");
    let manifest = dir.path().join("package.json");
    let pnpm = fs::read_to_string(&manifest)
        .unwrap()
        .replace("npm@11.6.2", "pnpm@12.15.0");
    fs::write(&manifest, pnpm).unwrap();
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let bin = shim("pnpm", &record, 0);

    let result = run(skif()
        .env("PATH", bin.path())
        .args(["--fix", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let calls = invocations(&record);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, "install --no-frozen-lockfile");
    assert_eq!(
        result.json()["fix"]["install"]["command"],
        "pnpm install --no-frozen-lockfile"
    );
}

#[test]
fn fix_with_sarif_keeps_stdout_a_single_document() {
    let dir = fixture("dirty");
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let bin = shim("npm", &record, 0);
    let result = run(skif()
        .env("PATH", bin.path())
        .args(["--fix", "--format", "sarif"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.json()["version"], "2.1.0");
    assert!(result.stderr.contains("Fixed "), "{}", result.stderr);
    assert_eq!(invocations(&record).len(), 1);
}

#[test]
fn fix_with_github_keeps_stdout_to_annotations() {
    let dir = fixture("dirty");
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let bin = shim("npm", &record, 0);
    let result = run(skif()
        .env("PATH", bin.path())
        .args(["--fix", "--format", "github"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(
        result
            .stdout
            .lines()
            .all(|line| line.starts_with("::") || line.starts_with("skif:")),
        "{}",
        result.stdout
    );
    assert!(result.stderr.contains("Fixed "), "{}", result.stderr);
}

#[test]
fn no_prompt_is_accepted_and_fixes_without_asking() {
    let dir = fixture("mismatch");
    let result = run(skif()
        .args(["--fix-unsafe", "--no-install", "--no-prompt", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert_eq!(is_number_versions(dir.path()), ["7.0.0", "7.0.0"]);
}

#[test]
fn sarif_paths_are_relative_to_the_repository_top_level() {
    let repo = fixture("dirty");
    fs::create_dir(repo.path().join(".git")).unwrap();
    let nested = repo.path().join("web");
    fs::create_dir(&nested).unwrap();
    let entries: Vec<PathBuf> = fs::read_dir(repo.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    for path in entries {
        if path.file_name().is_some_and(|name| name != ".git" && name != "web") {
            fs::rename(&path, nested.join(path.file_name().unwrap())).unwrap();
        }
    }
    let result = run(skif().args(["--format", "sarif"]).arg(&nested));
    let log = result.json();
    let uris: Vec<&str> = log["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| {
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_ne!(uris, [] as [&str; 0]);
    assert!(uris.iter().all(|uri| uri.starts_with("web/")), "{uris:?}");
}

#[test]
fn lint_to_json_returns_the_report_without_printing() {
    let dir = fixture("dirty");
    let text = skif_cli::lint_to_json(dir.path(), &skif::ConfigOverrides::default(), false).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["version"], 1);
    assert!(report["summary"]["errors"].as_u64().unwrap() > 0);
}

#[test]
fn lint_to_json_classifies_its_errors() {
    let missing = skif_cli::lint_to_json(
        Path::new("/definitely/not/here"),
        &skif::ConfigOverrides::default(),
        false,
    );
    assert_eq!(missing.unwrap_err().kind, skif_cli::Kind::Io);

    let dir = fixture("dirty");
    fs::write(dir.path().join("skif.toml"), "[lint\n").unwrap();
    let broken = skif_cli::lint_to_json(dir.path(), &skif::ConfigOverrides::default(), false).unwrap_err();
    assert_eq!(broken.kind, skif_cli::Kind::Config);
    assert!(broken.path.is_some_and(|path| path.ends_with("skif.toml")));
}

#[test]
fn list_shows_every_instance_by_version_group_and_exits_one_on_errors() {
    let dir = fixture("mismatch");
    let result = run(skif().args(["list", "--show", "all"]).arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    insta::assert_snapshot!("list_human", redact(&result.stdout, dir.path()));

    let ok = run(skif()
        .args(["list", "--show", "none", "--config"])
        .arg(dir.path().join("skif.toml"))
        .arg(dir.path().join("packages").join("a")));
    assert_eq!(ok.code, 1, "a member path lists its workspace: {}", ok.stdout);
}

#[test]
fn json_prints_one_instance_per_line_with_syncpack_status_codes() {
    let dir = fixture("mismatch");
    let result = run(skif().arg("json").arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    let lines: Vec<Value> = result
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let is_number: Vec<(&str, &str, &str)> = lines
        .iter()
        .filter(|line| line["dependency"] == "is-number")
        .map(|line| {
            (
                line["package"].as_str().unwrap(),
                line["statusCode"].as_str().unwrap(),
                line["statusType"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        is_number,
        [
            ("packages/a/package.json", "IsHighestOrLowestSemver", "Valid"),
            ("packages/b/package.json", "DiffersToHighestOrLowestSemver", "Fixable"),
        ]
    );
    insta::assert_snapshot!("json_instances", redact(&result.stdout, dir.path()));

    fs::write(
        dir.path().join("skif.toml"),
        "[js]\nenabled = false\n\n[rules]\nversion-mismatch = \"warn\"\n",
    )
    .unwrap();
    let warned = run(skif().arg("json").arg(dir.path()));
    assert_eq!(warned.code, 0, "warnings alone do not fail: {}", warned.stdout);
    assert!(warned.stdout.contains(r#""severity":"warning""#), "{}", warned.stdout);
}

#[test]
fn dry_run_shows_the_fix_plan_and_writes_nothing() {
    let dir = fixture("mismatch");
    let before = tree(dir.path());
    let human = run(skif()
        .args(["--fix-unsafe", "--no-prompt", "--dry-run"])
        .arg(dir.path()));
    assert_eq!(human.code, 1, "the report is of the unchanged tree: {}", human.stderr);
    assert_eq!(tree(dir.path()), before);
    insta::assert_snapshot!("fix_dry_run_human", redact(&human.stdout, dir.path()));

    let json = run(skif()
        .args(["--fix-unsafe", "--dry-run", "--format", "json"])
        .env("CI", "true")
        .arg(dir.path()));
    assert_eq!(json.code, 1, "a dry run is not refused in CI: {}", json.stdout);
    assert_eq!(tree(dir.path()), before);
    let fix = &json.json()["fix"];
    assert_eq!(fix["dry-run"], true);
    assert_eq!(fix["install"]["reason"], "--dry-run was given");
    insta::assert_snapshot!("fix_dry_run_json", serde_json::to_string_pretty(fix).unwrap());

    let out = tempfile::tempdir().unwrap();
    let report = out.path().join("report.json");
    let several = run(skif()
        .args([
            "--fix-unsafe",
            "--no-prompt",
            "--dry-run",
            "--format",
            "human",
            "--format",
        ])
        .arg(format!("json={}", report.display()))
        .arg(dir.path()));
    assert_eq!(several.code, 1, "{}", several.stderr);
    assert_eq!(tree(dir.path()), before, "no manifest is written with several formats");
    assert_eq!(several.stdout, human.stdout, "stdout holds the human rendering");
    let written: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(written["fix"]["dry-run"], true, "each FORMAT=FILE report is written");

    let alone = run(skif().args(["--dry-run", "--format", "json"]).arg(dir.path()));
    assert_eq!(alone.code, 2);
    assert_eq!(alone.json()["error"]["kind"], "usage");
}

#[test]
fn instance_filters_narrow_the_version_group_policies() {
    let dir = fixture("mismatch");
    fs::write(
        dir.path().join("skif.toml"),
        "[js]\nenabled = false\n\n[[version-groups]]\ndependencies = [\"left-pad\"]\npolicy = \"pinned\"\npin = \"1.3.0\"\n",
    )
    .unwrap();
    let all = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(
        rule_ids(&all.json()),
        BTreeSet::from(["policy-conflict".to_owned(), "version-mismatch".to_owned()])
    );

    let named = run(skif()
        .args(["--format", "json", "--dependency", "is-*"])
        .arg(dir.path()));
    assert_eq!(
        rule_ids(&named.json()),
        BTreeSet::from(["version-mismatch".to_owned()]),
        "a group the filter empties is not reported as unused"
    );

    let one_file = run(skif()
        .args(["--format", "json", "--source", "packages/b/package.json"])
        .arg(dir.path()));
    assert_eq!(one_file.code, 0, "nothing is left to align with: {}", one_file.stdout);

    let listed = run(skif()
        .args(["json", "--specifier-types", "exact", "--dependency-types", "prod"])
        .arg(dir.path()));
    let dependencies: BTreeSet<String> = listed
        .stdout
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["dependency"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(dependencies, BTreeSet::from(["is-number".to_owned()]));

    let unknown = run(skif()
        .args(["--format", "json", "--dependency-types", "engines"])
        .arg(dir.path()));
    assert_eq!(unknown.code, 2);
    assert_eq!(unknown.json()["error"]["kind"], "usage");
}

#[test]
fn format_writes_an_unsafe_exports_reorder_only_with_unsafe() {
    let dir = fixture("format");
    let manifest = dir.path().join("packages").join("a").join("package.json");
    fs::write(
        &manifest,
        r#"{"name":"@format/a","exports":{"import":"./a.mjs","module":"./a.esm.js"}}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("skif.toml"),
        "[format]\nsort-exports = [\"types\", \"module\", \"import\", \"default\"]\n",
    )
    .unwrap();
    let before = tree(dir.path());

    let safe = run(skif().args(["format"]).arg(dir.path()));
    assert_eq!(safe.code, 1, "{}{}", safe.stdout, safe.stderr);
    assert_eq!(
        tree(dir.path()),
        before,
        "an unsafe change was written without --unsafe"
    );

    let refused = run(skif().args(["format", "--check", "--unsafe"]).arg(dir.path()));
    assert_eq!(refused.code, 2, "{}{}", refused.stdout, refused.stderr);

    let applied = run(skif().args(["format", "--unsafe"]).arg(dir.path()));
    assert_eq!(applied.code, 0, "{}{}", applied.stdout, applied.stderr);
    assert_eq!(
        fs::read_to_string(&manifest).unwrap(),
        r#"{"name":"@format/a","exports":{"module":"./a.esm.js","import":"./a.mjs"}}"#
    );
}

#[test]
fn format_check_exits_one_and_format_writes_the_fix() {
    let dir = fixture("format");
    let manifest = dir.path().join("packages").join("a").join("package.json");
    let before = tree(dir.path());

    let check = run(skif().args(["format", "--check"]).arg(dir.path()));
    assert_eq!(check.code, 1, "{}{}", check.stdout, check.stderr);
    assert_eq!(tree(dir.path()), before, "--check wrote a file");
    insta::assert_snapshot!("format_check_human", redact(&check.stdout, dir.path()));

    let refused = run(skif().args(["format"]).arg(dir.path()).env("CI", "true"));
    assert_eq!(refused.code, 2, "{}{}", refused.stdout, refused.stderr);
    assert_eq!(tree(dir.path()), before, "a refused run wrote a file");

    let lint = run(skif().args(["--format", "json", "--no-js"]).arg(dir.path()));
    assert!(
        !rule_ids(&lint.json()).contains("package-json-format"),
        "the rule is off in a plain lint"
    );

    let format = run(skif().args(["format"]).arg(dir.path()));
    assert_eq!(format.code, 0, "{}{}", format.stdout, format.stderr);
    let formatted = fs::read_to_string(&manifest).unwrap();
    insta::assert_snapshot!("format_fixed_manifest", formatted);

    let again = run(skif().args(["format", "--check", "--format", "json"]).arg(dir.path()));
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(again.json()["summary"]["errors"], 0);
    let twice = run(skif().args(["format"]).arg(dir.path()));
    assert_eq!(twice.code, 0);
    assert_eq!(
        fs::read_to_string(&manifest).unwrap(),
        formatted,
        "formatting twice changed the file"
    );
}

#[test]
fn codeclimate_is_a_json_array_for_gitlab_and_can_go_to_output() {
    let dir = fixture("dirty");
    let stdout = run(skif().args(["--format", "codeclimate"]).arg(dir.path()));
    assert_eq!(stdout.code, 1);
    let issues = stdout.json();
    let issues = issues.as_array().unwrap();
    assert_ne!(issues.len(), 0);
    for issue in issues {
        assert!(issue["check_name"].is_string(), "{issue}");
        assert_eq!(issue["fingerprint"].as_str().unwrap().len(), 32, "{issue}");
        assert!(
            ["info", "minor", "major"].contains(&issue["severity"].as_str().unwrap()),
            "{issue}"
        );
        assert!(issue["location"]["lines"]["begin"].as_u64().unwrap() >= 1, "{issue}");
    }

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("gl-code-quality.json");
    let written = run(skif()
        .args(["--format", "codeclimate", "--output"])
        .arg(&target)
        .arg(dir.path()));
    assert_eq!(written.code, 1);
    assert!(written.stdout.is_empty(), "{}", written.stdout);
    let from_file: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(&from_file, &Value::Array(issues.clone()));
}

#[test]
fn compact_prints_one_line_per_diagnostic() {
    let dir = fixture("dirty");
    let compact = run(skif().args(["--format", "compact"]).arg(dir.path()));
    assert_eq!(compact.code, 1);
    let json = run(skif().args(["--format", "json"]).arg(dir.path())).json();
    let diagnostics = json["diagnostics"].as_array().unwrap();
    assert_eq!(compact.stdout.lines().count(), diagnostics.len(), "{}", compact.stdout);
    for (line, diagnostic) in compact.stdout.lines().zip(diagnostics) {
        let path = diagnostic["location"]["path"].as_str().unwrap();
        let rule = diagnostic["rule"].as_str().unwrap();
        assert!(line.starts_with(path), "{line}");
        assert!(line.contains(&format!(" {rule} ")), "{line}");
    }
}

#[test]
fn markdown_and_disclosure_group_by_rule() {
    let dir = fixture("dirty");
    let markdown = run(skif().args(["--format", "markdown"]).arg(dir.path()));
    assert_eq!(markdown.code, 1);
    assert!(markdown.stdout.starts_with("# skif report\n"), "{}", markdown.stdout);
    assert!(
        markdown.stdout.contains("| Location | Severity | Message |"),
        "{}",
        markdown.stdout
    );
    let disclosure = run(skif().args(["--format", "disclosure"]).arg(dir.path()));
    assert_eq!(disclosure.code, 1);
    assert_eq!(
        disclosure.stdout.matches("<details>").count(),
        markdown.stdout.matches("\n## ").count(),
        "{}",
        disclosure.stdout
    );
}

#[test]
fn codeowners_groups_by_the_repository_codeowners_file() {
    let dir = fixture("dirty");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".github")).unwrap();
    fs::write(dir.path().join("CODEOWNERS"), "* @ignored\n").unwrap();
    fs::write(
        dir.path().join(".github/CODEOWNERS"),
        "# owners\n* @org/core\n/package.json @org/root\n",
    )
    .unwrap();
    let result = run(skif().args(["--format", "codeowners"]).arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    let headings: Vec<&str> = result
        .stdout
        .lines()
        .filter(|line| line.starts_with('@') || line.starts_with('('))
        .collect();
    assert!(!headings.is_empty(), "{}", result.stdout);
    assert!(
        headings
            .iter()
            .all(|line| line.starts_with("@org/core (") || line.starts_with("@org/root (")),
        "{}",
        result.stdout
    );
}

#[test]
fn codeowners_without_a_codeowners_file_exits_two() {
    let dir = fixture("dirty");
    let result = run(skif().args(["--format", "codeowners"]).arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.is_empty(), "{}", result.stdout);
    assert!(result.stderr.contains("CODEOWNERS"), "{}", result.stderr);
}

#[test]
fn json_names_the_packages_whose_files_and_dependencies_were_not_judged() {
    let dir = tempfile::tempdir().unwrap();
    let put = |relative: &str, content: &str| {
        let path = dir.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    );
    put(
        "packages/web/package.json",
        r#"{"name":"web","version":"1.0.0","private":true,"main":"index.ts","devDependencies":{"vite":"6.0.0"}}"#,
    );
    put("packages/web/index.ts", "export const main = 1;\n");
    put("packages/web/orphan.ts", "export const lonely = 1;\n");
    put(
        "packages/web/vite.config.ts",
        "import base from './vite.base';\nexport default { ...base, build: { lib: { entry: pick() } } };\n",
    );
    put(
        "packages/api/package.json",
        r#"{"name":"api","version":"1.0.0","private":true,"main":"index.ts"}"#,
    );
    put("packages/api/index.ts", "export const api = 1;\n");
    put("packages/api/orphan.ts", "export const lonely = 1;\n");

    let json = run(skif().args(["--format", "json"]).arg(dir.path()));
    let document = json.json();
    assert_eq!(
        document["incomplete-packages"],
        serde_json::json!([{
            "name": "web",
            "dir": "packages/web",
            "reasons": [
                { "cause": "unknown-spread", "plugin": "vite", "file": "packages/web/vite.config.ts" },
                { "cause": "computed-entries", "plugin": "vite", "file": "packages/web/vite.config.ts" },
            ],
        }]),
        "{}",
        json.stdout
    );
    let unused_files: Vec<&str> = document["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["rule"] == "unused-files")
        .filter_map(|diagnostic| diagnostic["location"]["path"].as_str())
        .collect();
    assert_eq!(unused_files, ["packages/api/orphan.ts"]);

    let human = run(skif().arg(dir.path()));
    assert!(
        human
            .stdout
            .contains("note: unused files and dependencies were not fully checked in 1 package (web)"),
        "{}",
        human.stdout
    );
}

fn dependency_versions(dir: &Path, member: &str, section: &str) -> Value {
    let text = fs::read_to_string(dir.join("packages").join(member).join("package.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()[section].clone()
}

/// ADR 0051: `--fix` aligns a version within its semver window, whether the workspace or a pin chose it, and leaves
/// the cross-major move (also to a pin), the change of range and the move of type packages to `--fix-unsafe`.
#[test]
fn fix_applies_compatible_version_alignments_and_leaves_the_rest() {
    let dir = fixture("align");
    let result = run(skif()
        .args(["--fix", "--no-install", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);

    let a = dependency_versions(dir.path(), "a", "dependencies");
    let b = dependency_versions(dir.path(), "b", "dependencies");
    assert_eq!(
        (&a["left-pad"], &b["left-pad"]),
        (&Value::from("1.3.0"), &Value::from("1.3.0"))
    );
    assert_eq!(a["chalk"], "5.3.1", "a patch pin is applied");
    assert_eq!(a["react"], "18.3.1", "a cross-major pin is left to --fix-unsafe");
    assert_eq!(
        (&a["is-number"], &b["is-number"]),
        (&Value::from("6.0.0"), &Value::from("7.0.0"))
    );
    assert_eq!((&a["ms"], &b["ms"]), (&Value::from("^2.1.0"), &Value::from("2.1.3")));
    assert_eq!(a["@types/node"], "22.0.0", "type packages stay in dependencies");

    let report = result.json();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .flat_map(|diagnostic| diagnostic["fixes"].as_array().into_iter().flatten())
            .all(|fix| fix["safety"] != "safe"),
        "a safe fix survived --fix: {report}"
    );
    insta::assert_snapshot!("fix_align_json", redact(&result.stdout, dir.path()));
}

/// The files of `dir` that differ from the `autofix` fixture, with their new contents (`<deleted>` when gone), as
/// one text for a snapshot.
fn autofix_changes(dir: &Path) -> String {
    let before = tree(fixture("autofix").path());
    let after = tree(dir);
    let mut text = String::new();
    for path in changed(&before, &after) {
        let contents = after.get(&path).map_or("<deleted>\n", String::as_str);
        let shown = path.display().to_string().replace('\\', "/");
        text.push_str("--- ");
        text.push_str(&shown);
        text.push('\n');
        text.push_str(contents);
    }
    text
}

fn skipped_reasons(report: &Value) -> Vec<(String, String)> {
    report["fix"]["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skipped| {
            (
                skipped["rule"].as_str().unwrap().to_owned(),
                skipped["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Issue #37: `--fix-unsafe` removes enum and namespace members, `export default <expression>` and catalog entries in
/// place, but deletes no file without `--allow-remove-files`.
#[test]
fn fix_unsafe_deletes_files_only_with_allow_remove_files() {
    let dir = fixture("autofix");
    let result = run(skif()
        .args(["--fix-unsafe", "--no-install", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let report = result.json();
    assert!(dir.path().join("packages/app/src/orphan.ts").is_file());
    assert_eq!(
        skipped_reasons(&report),
        [("unused-files".to_owned(), "removes-files".to_owned())]
    );
    // `Size.Small` would renumber `Size.Large`, and `export default registerPlugin(...)` has a side effect: both stay
    // reported without a fix.
    let unfixable = ["unused-enum-members", "unused-exports"].map(str::to_owned);
    assert_eq!(
        rule_ids(&report),
        BTreeSet::from_iter(unfixable.iter().cloned().chain(["unused-files".to_owned()]))
    );
    insta::assert_snapshot!("autofix_fix_unsafe", autofix_changes(dir.path()));

    let human = fixture("autofix");
    let result = run(skif().args(["--fix-unsafe", "--no-install"]).arg(human.path()));
    assert!(
        result
            .stdout
            .contains("skipped [unused-files] `packages/app/src/orphan.ts` is not reachable from any entry file (removes-files: re-run with --allow-remove-files)"),
        "{}",
        result.stdout
    );

    let allowed = fixture("autofix");
    let result = run(skif()
        .args([
            "--fix-unsafe",
            "--allow-remove-files",
            "--no-install",
            "--format",
            "json",
        ])
        .arg(allowed.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(!allowed.path().join("packages/app/src/orphan.ts").exists());
    assert_eq!(rule_ids(&result.json()), BTreeSet::from(unfixable));
}

/// `--fix-type` takes knip issue types and rule ids, comma separated or repeated.
#[test]
fn fix_type_limits_the_fixes_to_some_rules() {
    let dir = fixture("autofix");
    let result = run(skif()
        .args(["--fix-unsafe", "--no-install", "--fix-type", "enumMembers,catalog"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    insta::assert_snapshot!("autofix_fix_type", autofix_changes(dir.path()));

    let dir = fixture("autofix");
    let result = run(skif()
        .args(["--fix-unsafe", "--no-install", "--allow-remove-files"])
        .args(["--fix-type", "files", "--fix-type", "unused-namespace-members"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let changes = autofix_changes(dir.path());
    assert!(
        changes.contains("--- packages/app/src/orphan.ts\n<deleted>"),
        "{changes}"
    );
    assert!(!changes.contains("pnpm-workspace.yaml"), "{changes}");
    assert!(changes.contains("  Green,"), "{changes}");
    assert!(!changes.contains("unused()"), "{changes}");
}

#[test]
fn fix_type_and_allow_remove_files_are_usage_errors_without_fix() {
    let dir = fixture("autofix");
    let unknown = run(skif()
        .args(["--fix-unsafe", "--fix-type", "nonsense", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(unknown.code, 2);
    let document = unknown.json();
    assert_eq!(document["error"]["kind"], "usage");
    assert!(
        document["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("unknown --fix-type `nonsense`"),
        "{document}"
    );
    assert_eq!(
        changed(&tree(fixture("autofix").path()), &tree(dir.path())),
        Vec::<PathBuf>::new()
    );

    for flags in [["--allow-remove-files", ""], ["--fix-type", "files"]] {
        let flags: Vec<&str> = flags.into_iter().filter(|flag| !flag.is_empty()).collect();
        let result = run(skif().args(&flags).args(["--format", "json"]).arg(dir.path()));
        assert_eq!(result.code, 2, "{flags:?}: {}", result.stdout);
        assert_eq!(result.json()["error"]["kind"], "usage", "{flags:?}");
    }
}
