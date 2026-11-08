//! `skif::update` end to end over an in-memory workspace and registry: plan, then the edits that write the plan.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif::update::{FetchError, Packument, Published, Registry, UpdateOptions, apply_updates, plan_updates};
use skif::{Config, FileChange, MemoryFs, UpdateTarget, Workspace};

const NOW: i64 = 1_767_225_600;

struct Npm;

impl Registry for Npm {
    fn fetch(&self, name: &str) -> Result<Packument, FetchError> {
        let old = Some(NOW - 30 * 86_400);
        match name {
            "react" => Ok(Packument::new(vec![
                Published::new("18.2.0", old),
                Published::new("18.3.1", old),
                Published::new("19.0.0", old),
                Published::new("19.1.0", Some(NOW - 3_600)),
            ])),
            "typescript" => Ok(Packument::new(vec![
                Published::new("5.4.0", old),
                Published::new("5.4.5", old),
                Published::new("5.6.3", old),
            ])),
            _ => Err(FetchError::Network("offline".to_owned())),
        }
    }
}

const ROOT: &str = "{\n  \"name\": \"root\",\n  \"private\": true,\n  \"workspaces\": [\"packages/*\"],\n  \"devDependencies\": {\n    \"typescript\": \"~5.4.0\"\n  }\n}\n";
const APP: &str = "{\n    \"name\": \"app\",\n    \"dependencies\": {\n        \"react\": \"^18.2.0\",\n        \"left-pad\": \"1.0.0\"\n    }\n}\n";

fn fixture() -> (MemoryFs, Config, Workspace) {
    let fs = MemoryFs::new()
        .with_file("/repo/package.json", ROOT)
        .with_file("/repo/packages/app/package.json", APP);
    let config =
        Config::from_toml_str("[[update-groups]]\ndependencies = [\"typescript\"]\ntarget = \"patch\"\n").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new("/repo"), &config, &fs).unwrap();
    (fs, config, workspace)
}

fn contents(change: &FileChange) -> (&Path, &str) {
    match change {
        FileChange::Modify { path, new_contents, .. } => (path, new_contents),
        other => panic!("unexpected change {other:?}"),
    }
}

#[test]
fn a_plan_writes_every_update_and_keeps_each_file_s_formatting() {
    let (fs, config, workspace) = fixture();
    let report = plan_updates(&workspace, &config, &Npm, &UpdateOptions::new(NOW));
    let plan = apply_updates(&workspace, &report, &fs).unwrap();
    assert_eq!(plan.changes.len(), 2);
    assert_eq!(
        contents(&plan.changes[0]),
        (Path::new("package.json"), ROOT.replace("~5.4.0", "~5.4.5").as_str()),
        "the update group caps typescript at patch"
    );
    assert_eq!(
        contents(&plan.changes[1]),
        (
            Path::new("packages/app/package.json"),
            APP.replace("^18.2.0", "^19.0.0").as_str()
        ),
        "19.1.0 is younger than a day"
    );
    assert_eq!(plan.applied.len(), 2);
    assert!(plan.applied.iter().all(|applied| applied.rule.as_str() == "update"));
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].dependency, "left-pad");
}

#[test]
fn a_run_target_narrows_every_instance() {
    let (fs, config, workspace) = fixture();
    let options = UpdateOptions::new(NOW).with_target(UpdateTarget::Minor);
    let report = plan_updates(&workspace, &config, &Npm, &options);
    let targets: Vec<(&str, &str)> = report
        .updates
        .iter()
        .map(|update| (update.dependency.as_str(), update.target.as_str()))
        .collect();
    assert_eq!(targets, [("typescript", "~5.4.5"), ("react", "^18.3.1")]);
    assert!(!apply_updates(&workspace, &report, &fs).unwrap().is_empty());
}
