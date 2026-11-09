//! End-to-end tests of multi-root and recursive linting.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{changed, fixture, invocations, run, shim, skif, tree};

fn strings(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

fn diagnostic_paths(report: &Value) -> BTreeSet<String> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["location"]["path"].as_str().unwrap().to_owned())
        .collect()
}

fn root_paths(report: &Value) -> Vec<String> {
    report["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| root["path"].as_str().unwrap().to_owned())
        .collect()
}

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

fn sibling_changes() -> BTreeSet<PathBuf> {
    BTreeSet::from([
        Path::new("solo").join("package.json"),
        Path::new("ws-one").join("package.json"),
        Path::new("ws-one").join("packages").join("a").join("package.json"),
    ])
}

#[test]
fn a_directory_without_a_manifest_lints_every_root_below_it() {
    let dir = fixture("multi/siblings");
    let (code, report) = json(dir.path(), &[]);
    assert_eq!(code, 1);
    assert_eq!(root_paths(&report), ["solo", "ws-one", "ws-two"]);

    let paths = diagnostic_paths(&report);
    for expected in [
        "solo/package.json",
        "ws-one/package.json",
        "ws-one/packages/a/package.json",
    ] {
        assert!(paths.contains(expected), "{expected} missing from {paths:?}");
    }
    assert!(paths.iter().all(|path| !path.starts_with("ws-two/")), "{paths:?}");
}

#[test]
fn recursive_reports_roots_with_their_own_summaries_and_an_aggregate() {
    let dir = fixture("multi/siblings");
    let (_, report) = json(dir.path(), &["--recursive"]);
    let roots = report["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 3);

    let packages: u64 = roots
        .iter()
        .map(|root| root["summary"]["packages"].as_u64().unwrap())
        .sum();
    assert_eq!(report["summary"]["packages"].as_u64().unwrap(), packages);
    for key in ["errors", "warnings", "infos", "fixable"] {
        let total: u64 = roots.iter().map(|root| root["summary"][key].as_u64().unwrap()).sum();
        assert_eq!(report["summary"][key].as_u64().unwrap(), total, "{key}");
    }

    let by_path = |path: &str| roots.iter().find(|root| root["path"] == path).unwrap();
    assert_eq!(strings(&by_path("ws-one")["packages"]), ["ws-one", "ws-one/packages/a"]);
    assert_eq!(strings(&by_path("solo")["packages"]), ["solo"]);
    assert_eq!(by_path("ws-two")["summary"]["errors"], 0);
    assert_eq!(by_path("ws-two")["summary"]["warnings"], 0);
    assert!(by_path("ws-one")["summary"]["errors"].as_u64().unwrap() > 0);
    assert_eq!(report["version"], 1);
}

#[test]
fn the_report_is_identical_across_runs() {
    let dir = fixture("multi/siblings");
    let first = run(skif().args(["--format", "json"]).arg(dir.path()));
    for _ in 0..4 {
        let again = run(skif().args(["--format", "json"]).arg(dir.path()));
        assert_eq!(again.stdout, first.stdout);
    }
}

#[test]
fn human_output_groups_diagnostics_by_root() {
    let dir = fixture("multi/siblings");
    let result = run(skif().arg(dir.path()));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    let position = |needle: &str| {
        result
            .stdout
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in {}", result.stdout))
    };
    let (solo, one, two) = (position("==> solo"), position("==> ws-one"), position("==> ws-two"));
    assert!(solo < one && one < two, "{}", result.stdout);
    assert!(position("ws-one/packages/a/package.json") > one);
    assert!(result.stdout.contains("Linted 3 roots."), "{}", result.stdout);
}

#[test]
fn sarif_and_github_concatenate_the_roots() {
    let dir = fixture("multi/siblings");
    let sarif = run(skif().args(["--format", "sarif"]).arg(dir.path()));
    assert_eq!(sarif.code, 1);
    let document = sarif.json();
    let uris: BTreeSet<String> = document["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| {
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(uris.iter().any(|uri| uri.ends_with("solo/package.json")), "{uris:?}");
    assert!(uris.iter().any(|uri| uri.ends_with("ws-one/package.json")), "{uris:?}");

    let github = run(skif().args(["--format", "github"]).arg(dir.path()));
    assert_eq!(github.code, 1);
    assert!(github.stdout.contains("solo/package.json"), "{}", github.stdout);
    assert!(
        github.stdout.contains("ws-one/packages/a/package.json"),
        "{}",
        github.stdout
    );
}

#[test]
fn a_directory_with_a_manifest_lints_only_its_workspace_unless_recursive() {
    let dir = fixture("multi/nested");
    let (_, single) = json(dir.path(), &[]);
    assert!(single.get("roots").is_none(), "{single}");
    assert!(diagnostic_paths(&single).iter().all(|path| !path.starts_with("tools/")));
    assert!(diagnostic_paths(&single).contains("packages/a/package.json"));

    let (code, recursive) = json(dir.path(), &["-r"]);
    // The fixtures report `empty-dependencies` as a warning, which doesn't fail the run.
    assert_eq!(code, 0);
    assert_eq!(root_paths(&recursive), [".", "tools"]);
    let paths = diagnostic_paths(&recursive);
    assert!(paths.contains("packages/a/package.json"), "{paths:?}");
    assert!(paths.contains("tools/p/t/package.json"), "{paths:?}");
}

#[test]
fn a_nested_root_inherits_the_configs_above_it_up_to_the_start() {
    let dir = fixture("multi/nested");
    let (_, inherited) = json(dir.path(), &["-r"]);
    let rules_in_tools: BTreeSet<String> = inherited["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["location"]["path"].as_str().unwrap().starts_with("tools/"))
        .map(|d| d["rule"].as_str().unwrap().to_owned())
        .collect();
    assert!(rules_in_tools.contains("empty-dependencies"), "{rules_in_tools:?}");

    fs::write(
        dir.path().join("tools").join("skif.toml"),
        "[rules]\nempty-dependencies = \"off\"\n\n[js]\nenabled = false\n",
    )
    .unwrap();
    let (_, own) = json(dir.path(), &["-r"]);
    assert!(
        diagnostic_paths(&own).iter().all(|path| !path.starts_with("tools/")),
        "the root's own config overrides what it inherits"
    );
}

#[test]
fn a_member_that_is_also_a_package_is_not_a_root() {
    let dir = fixture("multi/member-is-package");
    let (code, report) = json(dir.path(), &["-r"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(root_paths(&report), ["."]);
    assert_eq!(strings(&report["roots"][0]["packages"]), [".", "packages/a"]);
}

#[test]
fn a_path_inside_one_root_lints_only_that_root() {
    let dir = fixture("multi/siblings");
    let member = dir.path().join("ws-one").join("packages").join("a");
    let (code, report) = json(&member, &[]);
    assert_eq!(code, 1);
    assert!(report.get("roots").is_none());
    assert_eq!(report["summary"]["packages"], 2);
    assert_eq!(
        diagnostic_paths(&report),
        BTreeSet::from(["package.json".to_owned(), "packages/a/package.json".to_owned()])
    );

    let (code, solo) = json(&dir.path().join("solo"), &[]);
    // The fixtures report `empty-dependencies` as a warning, which doesn't fail the run.
    assert_eq!(code, 0);
    assert_eq!(solo["summary"]["packages"], 1);
    assert_eq!(diagnostic_paths(&solo), BTreeSet::from(["package.json".to_owned()]));
}

#[test]
fn an_ancestor_workspace_that_does_not_list_a_project_does_not_claim_it() {
    let dir = fixture("multi/unrelated");
    let (code, app) = json(&dir.path().join("app"), &[]);
    assert_eq!(code, 0);
    assert_eq!(app["summary"]["packages"], 1);
    assert_eq!(
        diagnostic_paths(&app),
        BTreeSet::from(["package.json".to_owned()]),
        "{app}"
    );

    let (_, member) = json(&dir.path().join("libs").join("l"), &[]);
    assert_eq!(member["summary"]["packages"], 2);

    let (_, recursive) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&recursive), [".", "app"]);
}

#[test]
fn a_pnpm_workspace_file_with_only_settings_is_a_single_package() {
    let dir = fixture("multi/pnpm-settings-only");
    let result = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let report = result.json();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["rule"] != "root-package-dependencies"),
        "{report}"
    );
    assert_eq!(report["summary"]["packages"], 1);
}

#[test]
fn the_exit_code_is_the_worst_across_roots() {
    let dir = fixture("multi/siblings");
    assert_eq!(run(skif().arg(dir.path().join("ws-two"))).code, 0);
    assert_eq!(run(skif().arg(dir.path())).code, 1);

    fs::write(dir.path().join("ws-two").join("skif.toml"), "[[").unwrap();
    let broken = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(broken.code, 2, "{}{}", broken.stdout, broken.stderr);
    let report = broken.json();
    assert_eq!(root_paths(&report), ["solo", "ws-one"]);
    assert_eq!(report["errors"][0]["path"], "ws-two");
    assert_eq!(report["errors"][0]["kind"], "config");
    assert!(broken.stderr.contains("error: ws-two:"), "{}", broken.stderr);
}

#[test]
fn fix_touches_only_the_roots_with_fixable_findings() {
    let dir = fixture("multi/siblings");
    let before = tree(dir.path());
    let result = run(skif()
        .args(["--fix", "--no-install", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);

    let after = tree(dir.path());
    assert_eq!(
        changed(&before, &after).into_iter().collect::<BTreeSet<_>>(),
        sibling_changes()
    );
    let report = result.json();
    let reported: BTreeSet<PathBuf> = strings(&report["fix"]["changed"])
        .into_iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(reported, sibling_changes());
    assert_eq!(report["fix"]["install"]["status"], "skipped");
    assert_eq!(report["summary"]["errors"], 0);
    assert_eq!(root_paths(&report), ["solo", "ws-one", "ws-two"]);

    let member: Value =
        serde_json::from_str(&after[&Path::new("ws-one").join("packages").join("a").join("package.json")]).unwrap();
    assert!(member.get("devDependencies").is_none(), "{member}");
}

#[test]
fn fix_in_human_output_names_each_root() {
    let dir = fixture("multi/siblings");
    let result = run(skif().args(["--fix", "--no-install"]).arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.contains("ws-one:\nFixed"), "{}", result.stdout);
    assert!(result.stdout.contains("  fixed solo/package.json"), "{}", result.stdout);
}

#[test]
fn install_runs_in_each_fixed_root_with_its_own_package_manager() {
    let dir = fixture("multi/siblings");
    let log = tempfile::tempdir().unwrap();
    let record = log.path().join("record.txt");
    let npm = shim("npm", &record, 0);
    let pnpm = shim("pnpm", &record, 0);
    let path: OsString = std::env::join_paths([npm.path(), pnpm.path()]).unwrap();

    let result = run(skif()
        .env("PATH", path)
        .args(["--fix", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.json()["fix"]["install"]["status"], "ran");

    let calls: BTreeSet<(String, PathBuf)> = invocations(&record)
        .into_iter()
        .map(|(argv, cwd)| (argv, cwd.canonicalize().unwrap()))
        .collect();
    let root = dir.path().canonicalize().unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls.contains(&("install".to_owned(), root.join("solo"))), "{calls:?}");
    assert!(
        calls.contains(&("install --no-frozen-lockfile".to_owned(), root.join("ws-one"))),
        "{calls:?}"
    );
}

#[test]
fn a_directory_with_nothing_to_lint_is_a_workspace_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("readme.md"), "").unwrap();
    let result = run(skif().args(["--format", "json"]).arg(dir.path()));
    assert_eq!(result.code, 2);
    assert_eq!(result.json()["error"]["kind"], "workspace");
}

#[test]
fn lint_to_json_supports_recursion() {
    let dir = fixture("multi/siblings");
    let text = skif_cli::lint_to_json(dir.path(), &skif::ConfigOverrides::default(), true).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(root_paths(&report), ["solo", "ws-one", "ws-two"]);

    let single = skif_cli::lint_to_json(&dir.path().join("solo"), &skif::ConfigOverrides::default(), false).unwrap();
    let report: Value = serde_json::from_str(&single).unwrap();
    assert!(report.get("roots").is_none());
}

fn put(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn package(name: &str) -> String {
    format!(r#"{{"name": "{name}", "version": "0.0.0", "private": true}}"#)
}

fn workspace(name: &str, patterns: &str) -> String {
    format!(r#"{{"name": "{name}", "version": "0.0.0", "private": true, "workspaces": [{patterns}]}}"#)
}

fn packages_of(report: &Value, root: &str) -> Vec<String> {
    let entry = report["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == root)
        .unwrap_or_else(|| panic!("{root} in {report}"));
    strings(&entry["packages"])
}

#[test]
fn a_manifest_above_the_repository_does_not_hijack_the_run() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "package.json", &package("stray"));
    fs::create_dir_all(dir.path().join("repo").join(".git")).unwrap();
    put(dir.path(), "repo/a/package.json", &package("a"));
    put(dir.path(), "repo/b/package.json", &package("b"));
    let (_, report) = json(&dir.path().join("repo"), &[]);
    assert_eq!(root_paths(&report), ["a", "b"]);
}

#[test]
fn roots_are_claimed_with_each_workspace_s_configured_package_manager() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "package.json", &workspace("root", r#""npmpkgs/*""#));
    put(dir.path(), "pnpm-workspace.yaml", "packages:\n  - 'yarnpkgs/*'\n");
    put(dir.path(), "skif.toml", "[workspace]\npackage-manager = \"npm\"\n");
    put(dir.path(), "npmpkgs/x/package.json", &package("x"));
    put(dir.path(), "yarnpkgs/y/package.json", &package("y"));
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), [".", "yarnpkgs/y"]);
    assert_eq!(packages_of(&report, "."), [".", "npmpkgs/x"]);
}

#[test]
fn a_nested_workspace_is_a_root_and_every_package_is_linted_once() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "package.json", &workspace("root", r#""packages/*""#));
    put(
        dir.path(),
        "packages/inner/package.json",
        &workspace("inner", r#""x/*""#),
    );
    put(dir.path(), "packages/inner/x/y/package.json", &package("y"));
    put(dir.path(), "packages/plain/package.json", &package("plain"));
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), [".", "packages/inner"]);
    assert_eq!(packages_of(&report, "."), [".", "packages/plain"]);
    assert_eq!(
        packages_of(&report, "packages/inner"),
        ["packages/inner", "packages/inner/x/y"]
    );

    let (_, direct) = json(&dir.path().join("packages").join("inner"), &[]);
    assert!(direct.get("roots").is_none());
    assert_eq!(direct["summary"]["packages"], 2);
}

#[cfg(unix)]
#[test]
fn a_symlinked_member_is_linted_once() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "package.json", &workspace("root", r#""pk/*""#));
    put(dir.path(), "store/m/package.json", &package("m"));
    fs::create_dir_all(dir.path().join("pk")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("store").join("m"), dir.path().join("pk").join("m")).unwrap();
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), ["."]);
    assert_eq!(packages_of(&report, "."), [".", "pk/m"]);
}

#[test]
fn ignore_files_above_the_start_apply_inside_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(dir.path(), ".gitignore", "dist/\n");
    put(dir.path(), "sub/app/package.json", &package("app"));
    put(dir.path(), "sub/dist/package.json", &package("dist"));
    let (_, report) = json(&dir.path().join("sub"), &["-r"]);
    assert_eq!(root_paths(&report), ["app"]);
}

#[test]
fn a_walk_cut_by_the_depth_limit_says_so() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a/package.json", &package("a"));
    let deep: Vec<String> = (0..34).map(|level| format!("d{level}")).collect();
    put(
        dir.path(),
        &format!("{}/package.json", deep.join("/")),
        &package("deep"),
    );
    let result = run(skif().args(["--format", "json", "-r"]).arg(dir.path()));
    assert!(result.code <= 1, "{}{}", result.stdout, result.stderr);
    assert!(
        result
            .stderr
            .contains("warning: the search for roots stopped 32 levels down"),
        "{}",
        result.stderr
    );
    let report = result.json();
    assert_eq!(root_paths(&report), ["a"]);
    let truncated = strings(&report["truncated"]);
    assert_eq!(truncated.len(), 1, "{report}");
    assert!(truncated[0].starts_with("d0/d1/d2/"), "{truncated:?}");
}

#[test]
fn a_walk_that_reaches_every_root_has_no_truncated_key() {
    let dir = fixture("multi/siblings");
    let (_, report) = json(dir.path(), &[]);
    assert!(report.get("truncated").is_none(), "{report}");
}

#[test]
fn a_root_that_cannot_be_loaded_does_not_stop_the_others() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "good/package.json", &package("good"));
    put(dir.path(), "bad/package.json", "{");
    put(dir.path(), "ws/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n");
    put(dir.path(), "ws/package.json", "{");
    put(dir.path(), "ws/p/m/package.json", &package("m"));
    let result = run(skif().args(["--format", "json", "-r"]).arg(dir.path()));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    let report = result.json();
    assert_eq!(root_paths(&report), ["good"]);
    let failed: Vec<&str> = report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(failed, ["bad", "ws"]);
}

#[test]
fn a_project_is_judged_by_the_same_config_whichever_way_it_is_reached() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(dir.path(), "I/skif.toml", "[rules]\nempty-dependencies = \"off\"\n");
    put(dir.path(), "I/package.json", &workspace("i", r#""pkgs/*""#));
    put(dir.path(), "I/pkgs/a/package.json", &package("a"));
    put(
        dir.path(),
        "I/examples/e/package.json",
        r#"{"name": "e", "version": "0.0.0", "private": true, "devDependencies": {}}"#,
    );
    let rules = |report: &Value, path: &str| -> BTreeSet<String> {
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["location"]["path"] == path)
            .map(|d| d["rule"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, recursive) = json(&dir.path().join("I"), &["-r"]);
    let (_, direct) = json(&dir.path().join("I").join("examples").join("e"), &[]);
    let through_the_tree = rules(&recursive, "examples/e/package.json");
    assert!(!through_the_tree.contains("empty-dependencies"), "{recursive}");
    assert_eq!(rules(&direct, "package.json"), through_the_tree, "{direct}");
}

#[cfg(unix)]
#[test]
fn a_write_failure_in_a_later_root_still_reports_the_earlier_fixes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = fixture("multi/siblings");
    let locked = dir.path().join("ws-one");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let writable = fs::write(locked.join("probe"), "").is_ok();
    let result = run(skif().args(["--fix", "--no-install"]).arg(dir.path()));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        return;
    }
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.contains("  fixed solo/package.json"), "{}", result.stdout);
    assert!(
        result.stderr.contains("fixes were written to earlier roots"),
        "{}",
        result.stderr
    );
}

fn paths_reporting(report: &Value, rule: &str) -> BTreeSet<String> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"] == rule)
        .map(|d| d["location"]["path"].as_str().unwrap().to_owned())
        .collect()
}

fn error_count(report: &Value) -> usize {
    report.get("errors").and_then(Value::as_array).map_or(0, Vec::len)
}

#[test]
fn the_package_manager_field_is_asked_of_workspaces_and_the_outermost_root_only() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "package.json", &workspace("root", r#""packages/*""#));
    put(dir.path(), "packages/a/package.json", &package("a"));
    put(dir.path(), "examples/e/package.json", &package("e"));
    put(dir.path(), "libs/w/package.json", &workspace("w", r#""p/*""#));
    put(dir.path(), "libs/w/p/m/package.json", &package("m"));
    put(dir.path(), "libs/w/demo/package.json", &package("demo"));
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), [".", "examples/e", "libs/w", "libs/w/demo"]);
    assert_eq!(
        paths_reporting(&report, "root-package-manager-field"),
        BTreeSet::from(["package.json".to_owned(), "libs/w/package.json".to_owned()]),
        "{report}"
    );

    let (_, single) = json(&dir.path().join("examples").join("e"), &[]);
    assert_eq!(
        paths_reporting(&single, "root-package-manager-field"),
        BTreeSet::from(["package.json".to_owned()]),
        "a package linted on its own is its own outermost root"
    );
}

#[test]
fn standalone_packages_without_a_common_root_each_get_the_package_manager_check() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a/package.json", &package("a"));
    put(dir.path(), "b/package.json", &package("b"));
    put(dir.path(), "b/inner/package.json", &package("inner"));
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(
        paths_reporting(&report, "root-package-manager-field"),
        BTreeSet::from(["a/package.json".to_owned(), "b/package.json".to_owned()]),
        "{report}"
    );
}

#[test]
fn a_workspace_file_without_a_root_manifest_lints_its_members() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "ws/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n");
    put(dir.path(), "ws/p/m/package.json", &package("m"));
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(error_count(&report), 0, "{report}");
    assert_eq!(root_paths(&report), ["ws"]);
    assert_eq!(packages_of(&report, "ws"), ["ws/p/m"]);
    assert_eq!(report["summary"]["packages"], 1);
    assert!(
        diagnostic_paths(&report).iter().all(|path| path != "ws/package.json"),
        "{report}"
    );

    let (_, direct) = json(&dir.path().join("ws"), &[]);
    assert!(direct.get("roots").is_none());
    assert_eq!(direct["summary"]["packages"], 1);
}

#[test]
fn a_rush_json_makes_its_projects_the_members_of_one_root() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "rush.json",
        "// Rush\n{\n  \"pnpmVersion\": \"9.0.0\",\n  \"projects\": [\n    { \"packageName\": \"a\", \"projectFolder\": \"apps/a\" },\n    { \"packageName\": \"b\", \"projectFolder\": \"libs/b\" },\n  ],\n}\n",
    );
    put(dir.path(), "apps/a/package.json", &package("a"));
    put(dir.path(), "libs/b/package.json", &package("b"));
    put(dir.path(), "libs/b/example/package.json", &package("example"));
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(error_count(&report), 0, "{report}");
    assert_eq!(root_paths(&report), ["."]);
    assert_eq!(packages_of(&report, "."), ["apps/a", "libs/b"]);
}

#[test]
fn fixture_directories_are_not_searched_for_roots_unless_configured() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "real/package.json", &package("real"));
    put(dir.path(), "fixtures/f/package.json", &package("f"));
    put(dir.path(), "real/__mocks__/m/package.json", &package("m"));
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), ["real"]);

    put(dir.path(), "skif.toml", "[workspace]\ndiscover-ignore = []\n");
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), ["fixtures/f", "real", "real/__mocks__/m"]);

    put(dir.path(), "skif.toml", "[workspace]\ndiscover-ignore = [\"real\"]\n");
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), ["fixtures/f"]);
}

/// The JS findings of a report as `rule path: message`, in report order.
fn js_findings(report: &Value) -> Vec<String> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| matches!(d["rule"].as_str(), Some("unused-files" | "unused-exports")))
        .map(|d| {
            format!(
                "{} {}: {}",
                d["rule"].as_str().unwrap(),
                d["location"]["path"].as_str().unwrap(),
                d["message"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn a_file_of_a_nested_root_that_an_outer_root_imports_is_its_entry() {
    // `src/main.ts` of the outer root imports `build/lib/x.ts`, a file of the nested root `build`; `x.ts` imports
    // `y.ts`, whose `spare` nothing uses, and nothing imports `orphan.ts`.
    let dir = fixture("multi/nested-imports");
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), [".", "build"]);
    insta::assert_snapshot!("nested_imports_recursive", js_findings(&report).join("\n"));

    // Linted on its own, the nested root cannot know what the outer root imports.
    let (_, alone) = json(&dir.path().join("build"), &[]);
    let alone = js_findings(&alone);
    assert!(
        alone.iter().any(|finding| finding.starts_with("unused-files lib/x.ts")),
        "{alone:?}"
    );
}

#[test]
fn the_lint_after_fixing_judges_each_root_as_the_first_lint_did() {
    // `packages/inner` is a member of the outer workspace and a workspace of its own, so the outer root leaves it to
    // its own run; `build` is a standalone package below the outer root, which needs no `packageManager`.
    let dir = tempfile::tempdir().unwrap();
    let empty_dev = |name: &str| {
        format!(
            r#"{{"name": "{name}", "version": "0.0.0", "private": true, "packageManager": "pnpm@9.0.0",
                "devDependencies": {{}}}}"#
        )
    };
    put(dir.path(), "package.json", &empty_dev("outer"));
    put(dir.path(), "pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n");
    put(dir.path(), "packages/a/package.json", &package("a"));
    put(dir.path(), "packages/inner/package.json", &empty_dev("inner"));
    put(
        dir.path(),
        "packages/inner/pnpm-workspace.yaml",
        "packages:\n  - \"libs/*\"\n",
    );
    put(dir.path(), "packages/inner/libs/x/package.json", &package("x"));
    put(
        dir.path(),
        "build/package.json",
        r#"{"name": "build", "version": "0.0.0", "private": true, "devDependencies": {}}"#,
    );
    let result = run(skif()
        .args(["-r", "--fix", "--no-install", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let report = result.json();
    assert_eq!(root_paths(&report), [".", "build", "packages/inner"]);
    assert_eq!(packages_of(&report, "."), [".", "packages/a"]);
    assert_eq!(report["diagnostics"], Value::Array(Vec::new()), "{report}");
}

#[test]
fn fixing_keeps_the_files_an_outer_root_imports() {
    // Deleting unused files and exports must not touch `x.ts` (imported by the outer root) or `helper` of `y.ts`,
    // and the lint after fixing still makes `x.ts` an entry of `build`.
    let dir = fixture("multi/nested-imports");
    let result = run(skif()
        .args([
            "-r",
            "--fix-unsafe",
            "--allow-remove-files",
            "--no-install",
            "--format",
            "json",
        ])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let lib = dir.path().join("build").join("lib");
    assert!(!lib.join("orphan.ts").exists());
    assert!(lib.join("x.ts").exists());
    let y = fs::read_to_string(lib.join("y.ts")).unwrap();
    assert!(
        y.contains("export function helper") && !y.contains("export const spare"),
        "{y}"
    );
    let report = result.json();
    assert_eq!(js_findings(&report), Vec::<String>::new());
}

/// The unused-dependency findings of a report as `path: name`, sorted.
fn unused_dependencies(report: &Value) -> Vec<String> {
    let mut found: Vec<String> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| matches!(d["rule"].as_str(), Some("unused-dependencies" | "unused-dev-dependencies")))
        .map(|d| {
            format!(
                "{}: {}",
                d["location"]["path"].as_str().unwrap(),
                d["dependency"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    found
}

/// An outer root that lists `left-pad`, `gulp` and `spare`, and a nested root `build` that lists none of them but
/// imports `left-pad` and runs `gulp`, which Node.js and the shell find in the outer root's `node_modules`.
fn outer_with_nested_user() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    put(
        dir.path(),
        "package.json",
        r#"{"name": "outer", "version": "0.0.0", "private": true, "packageManager": "npm@11.6.2",
            "dependencies": {"left-pad": "1.0.0"}, "devDependencies": {"gulp": "4.0.0", "spare": "1.0.0"}}"#,
    );
    put(dir.path(), "index.js", "console.log(1);\n");
    put(
        dir.path(),
        "build/package.json",
        r#"{"name": "build", "version": "0.0.0", "private": true, "main": "index.js", "scripts": {"build": "gulp"}}"#,
    );
    put(dir.path(), "build/index.js", "module.exports = require('left-pad');\n");
    dir
}

#[test]
fn a_dependency_only_a_nested_root_uses_is_used_by_the_outer_manifest() {
    let dir = outer_with_nested_user();
    let (_, report) = json(dir.path(), &["-r"]);
    assert_eq!(root_paths(&report), [".", "build"]);
    assert_eq!(unused_dependencies(&report), ["package.json: spare"], "{report}");

    // Linted on its own, the outer root cannot see the code of the nested root.
    let (_, alone) = json(dir.path(), &[]);
    assert_eq!(
        unused_dependencies(&alone),
        ["package.json: gulp", "package.json: left-pad", "package.json: spare"],
        "{alone}"
    );
}

#[test]
fn fixing_keeps_the_dependencies_a_nested_root_uses() {
    let dir = outer_with_nested_user();
    let result = run(skif()
        .args(["-r", "--fix-unsafe", "--no-install", "--format", "json"])
        .arg(dir.path()));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    let manifest = fs::read_to_string(dir.path().join("package.json")).unwrap();
    assert!(
        manifest.contains("left-pad") && manifest.contains("gulp") && !manifest.contains("spare"),
        "{manifest}"
    );
    assert_eq!(unused_dependencies(&result.json()), Vec::<String>::new());
}
