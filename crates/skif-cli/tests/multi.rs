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
fn a_nested_root_inherits_the_nearest_config_above_it_up_to_the_start() {
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
        "the root's own config replaces the inherited one"
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
    assert_eq!(broken.json()["error"]["kind"], "config");
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
