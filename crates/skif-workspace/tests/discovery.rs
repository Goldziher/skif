#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use skif_config::Config;
use skif_workspace::{PackageManager, Workspace, WorkspaceError, WorkspaceSource};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
}

fn discover(name: &str) -> Workspace {
    Workspace::discover(&fixture(name), &Config::default()).unwrap()
}

fn dirs(workspace: &Workspace) -> Vec<String> {
    workspace
        .packages
        .iter()
        .map(|p| p.dir.to_str().unwrap().to_owned())
        .collect()
}

fn names(workspace: &Workspace) -> Vec<String> {
    let mut names: Vec<String> = workspace.packages.iter().filter_map(|p| p.name.clone()).collect();
    names.sort();
    names
}

fn unmatched(workspace: &Workspace) -> Vec<&str> {
    workspace
        .facts
        .patterns
        .iter()
        .filter(|fact| fact.matched == 0)
        .map(|fact| fact.pattern.as_str())
        .collect()
}

#[test]
fn unknown_dir() {
    let error = Workspace::discover(Path::new("unknown"), &Config::default()).unwrap_err();
    assert!(matches!(error, WorkspaceError::NotADirectory(_)));
}

#[test]
fn empty_dir_has_no_root_manifest() {
    let error = Workspace::discover(&fixture("empty"), &Config::default()).unwrap_err();
    assert!(matches!(error, WorkspaceError::MissingRootManifest(_)));
}

#[test]
fn basic() {
    let workspace = discover("basic");
    assert_eq!(workspace.root_package.name.as_deref(), Some("basic"));
    assert!(workspace.root_package.is_root);
    assert_eq!(dirs(&workspace), ["docs", "packages/abc", "packages/def"]);
    assert_eq!(workspace.facts.source, WorkspaceSource::PackageJsonWorkspaces);
    assert_eq!(unmatched(&workspace), ["examples/*", "website"]);
    assert_eq!(workspace.facts.dirs_without_manifest, [] as [std::path::PathBuf; 0]);
    assert!(workspace.root.is_absolute());
}

#[test]
fn pnpm() {
    let workspace = discover("pnpm");
    assert_eq!(workspace.facts.source, WorkspaceSource::PnpmWorkspaceYaml);
    assert_eq!(workspace.package_manager, Some(PackageManager::Pnpm));
    assert_eq!(dirs(&workspace), ["docs", "packages/abc", "packages/def"]);
    assert_eq!(unmatched(&workspace), ["examples/*", "website"]);
    let span = workspace.facts.patterns[0].span.unwrap();
    let yaml = workspace.pnpm_workspace.as_ref().unwrap();
    assert_eq!(&yaml.source[span.start as usize..span.end as usize], "'packages/*'");
}

#[test]
fn yarn_nohoist() {
    let workspace = discover("yarn-nohoist");
    assert_eq!(workspace.facts.source, WorkspaceSource::YarnNohoistWorkspaces);
    assert_eq!(dirs(&workspace), ["docs", "packages/abc", "packages/def"]);
    assert_eq!(unmatched(&workspace), ["examples/*", "website"]);
}

#[test]
fn no_workspace_pnpm() {
    let error = Workspace::discover(&fixture("no-workspace-pnpm"), &Config::default()).unwrap_err();
    assert!(matches!(error, WorkspaceError::NoWorkspaces(_)));
}

#[test]
fn directories_without_manifest_are_facts() {
    let workspace = discover("without-package-json");
    assert_eq!(dirs(&workspace), ["packages/def"]);
    assert_eq!(
        workspace.facts.dirs_without_manifest,
        [PathBuf::from("docs"), PathBuf::from("packages/abc")]
    );
}

#[test]
fn empty_directories_are_skipped() {
    let root = std::env::temp_dir().join(format!("skif-workspace-empty-{}", std::process::id()));
    std::fs::create_dir_all(root.join("packages/abc")).unwrap();
    std::fs::create_dir_all(root.join("packages/empty")).unwrap();
    std::fs::write(root.join("package.json"), r#"{"name":"r","workspaces":["packages/*"]}"#).unwrap();
    std::fs::write(root.join("packages/abc/package.json"), r#"{"name":"abc"}"#).unwrap();

    let result = Workspace::discover(&root, &Config::default());
    std::fs::remove_dir_all(&root).unwrap();

    let workspace = result.unwrap();
    assert_eq!(dirs(&workspace), ["packages/abc"]);
    assert_eq!(workspace.facts.dirs_without_manifest, [] as [std::path::PathBuf; 0]);
    assert_eq!(unmatched(&workspace), [] as [&str; 0]);
}

#[test]
fn negations_use_whole_paths() {
    let workspace = discover("ignore-paths");
    assert_eq!(names(&workspace), ["d", "docs", "e", "ghi"]);
    assert_eq!(workspace.facts.dirs_without_manifest, [PathBuf::from("packages/a")]);
    let negations: Vec<(&str, usize)> = workspace
        .facts
        .patterns
        .iter()
        .filter(|fact| fact.negated)
        .map(|fact| (fact.pattern.as_str(), fact.matched))
        .collect();
    assert_eq!(
        negations,
        [("!packages/abc", 1), ("!packages/d*", 1), ("!packages/a/*", 0)]
    );
}

#[test]
fn pnpm_glob_with_prefix_and_hidden_dirs() {
    let workspace = discover("pnpm-glob");
    assert_eq!(dirs(&workspace), ["@ui", "@web"]);
    assert_eq!(unmatched(&workspace), [] as [&str; 0]);
}

#[test]
fn root_star_matches_direct_children_with_manifest() {
    let workspace = discover("root-star-pnpm");
    assert_eq!(names(&workspace), ["pk-a", "tool"]);
    assert_eq!(unmatched(&workspace), [] as [&str; 0]);
    assert_eq!(workspace.facts.dirs_without_manifest, [] as [std::path::PathBuf; 0]);
}

#[test]
fn deeply_nested_double_star() {
    let workspace = discover("nested-star");
    assert_eq!(
        names(&workspace),
        ["@repo/level1", "@repo/level2", "@repo/level3", "@repo/level4"]
    );
    assert_eq!(
        dirs(&workspace),
        [
            "packages/level1",
            "packages/level2/level2",
            "packages/level3/level3/level3",
            "packages/level4/level4/level4/level4"
        ]
    );
}

#[test]
fn pnpm_catalogs() {
    let workspace = discover("catalogs");
    assert_eq!(workspace.catalogs.resolve("typescript", "catalog:"), Some("5.9.3"));
    assert_eq!(workspace.catalogs.resolve("typescript", "catalog:ts6"), Some("6.0.3"));
    assert_eq!(workspace.catalogs.source(), Some(Path::new("pnpm-workspace.yaml")));
    assert_eq!(workspace.package_manager, Some(PackageManager::Pnpm));
}

#[test]
fn package_json_catalogs() {
    let workspace = discover("catalogs-package-json");
    assert_eq!(workspace.catalogs.resolve("typescript", "catalog:"), Some("5.9.3"));
    assert_eq!(
        workspace.catalogs.resolve("typescript", "catalog:legacy"),
        Some("5.8.0")
    );
    assert_eq!(workspace.catalogs.source(), Some(Path::new("package.json")));
}

#[test]
fn dev_engines_object_array_and_missing() {
    assert_eq!(discover("dev-engines").package_manager, Some(PackageManager::Pnpm));
    assert_eq!(discover("dev-engines-array").package_manager, Some(PackageManager::Bun));
    assert_eq!(discover("dev-engines-no-package-manager").package_manager, None);
}

#[test]
fn dependency_spans_in_fixture() {
    let workspace = discover("catalogs");
    let a = workspace.package_by_name("a").unwrap();
    let entry = a.dependencies.iter().find(|d| d.name == "typescript").unwrap();
    let source = &a.manifest.source;
    assert_eq!(
        &source[entry.value_span.start as usize..entry.value_span.end as usize],
        "\"catalog:\""
    );
    assert_eq!(a.manifest.path, PathBuf::from("a/package.json"));
}

#[test]
fn root_given_with_dot_segments_reports_relative_slash_paths() {
    let root = fixture("basic").join("packages").join("..");
    let workspace = Workspace::discover(&root, &Config::default()).unwrap();
    assert_eq!(dirs(&workspace), ["docs", "packages/abc", "packages/def"]);
}

#[cfg(unix)]
#[test]
fn double_star_does_not_descend_symbolic_links() {
    let root = std::env::temp_dir().join(format!("skif-workspace-symlink-{}", std::process::id()));
    std::fs::create_dir_all(root.join("packages/a")).unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"r","workspaces":["packages/**"]}"#,
    )
    .unwrap();
    std::fs::write(root.join("packages/a/package.json"), r#"{"name":"a"}"#).unwrap();
    std::os::unix::fs::symlink(root.join("packages"), root.join("packages/a/loop")).unwrap();

    let result = Workspace::discover(&root, &Config::default());
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(dirs(&result.unwrap()), ["packages/a"]);
}

#[test]
fn discover_resolves_a_relative_root_against_the_current_directory() {
    let relative = Path::new("fixtures").join("basic");
    let workspace = Workspace::discover(&relative, &Config::default()).unwrap();
    assert!(workspace.root.is_absolute());
    assert!(workspace.root.ends_with(&relative));
}
