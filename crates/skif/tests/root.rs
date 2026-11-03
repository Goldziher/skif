#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif::{Error, MemoryFs, find_workspace_root};

#[test]
fn finds_the_nearest_declaring_ancestor() {
    let fs = MemoryFs::new()
        .with_file("/outer/package.json", r#"{"workspaces": ["*"]}"#)
        .with_file("/outer/inner/package.json", r#"{"workspaces": ["pkgs/*"]}"#)
        .with_file("/outer/inner/pkgs/a/package.json", r#"{"name": "a"}"#)
        .with_file("/outer/inner/pkgs/a/src/index.ts", "");
    for start in [
        "/outer/inner/pkgs/a",
        "/outer/inner/pkgs/a/src",
        "/outer/inner/pkgs/a/src/index.ts",
        "/outer/inner",
    ] {
        assert_eq!(
            find_workspace_root(Path::new(start), &fs).unwrap(),
            Path::new("/outer/inner"),
            "{start}"
        );
    }
}

#[test]
fn pnpm_workspace_file_declares_a_workspace() {
    let fs = MemoryFs::new()
        .with_file("/repo/pnpm-workspace.yaml", "packages:\n  - 'pkgs/*'\n")
        .with_file("/repo/package.json", "{}")
        .with_file("/repo/pkgs/a/package.json", r#"{"name": "a"}"#);
    assert_eq!(
        find_workspace_root(Path::new("/repo/pkgs/a"), &fs).unwrap(),
        Path::new("/repo")
    );
}

#[test]
fn a_package_without_workspaces_is_returned_so_discovery_can_explain() {
    let fs = MemoryFs::new().with_file("/solo/package.json", r#"{"name": "solo"}"#);
    assert_eq!(
        find_workspace_root(Path::new("/solo"), &fs).unwrap(),
        Path::new("/solo")
    );
}

#[test]
fn nothing_declaring_a_workspace_is_an_error() {
    let fs = MemoryFs::new().with_file("/empty/readme.md", "");
    let error = find_workspace_root(Path::new("/empty"), &fs).unwrap_err();
    assert!(matches!(error, Error::NoWorkspace(_)), "{error}");
}

#[test]
fn relative_start_paths_are_rejected() {
    let fs = MemoryFs::new().with_file("/repo/package.json", r#"{"workspaces": ["*"]}"#);
    for start in ["", ".", "packages/a", "../repo"] {
        let error = find_workspace_root(Path::new(start), &fs).unwrap_err();
        assert!(matches!(error, Error::RelativePath(_)), "{start}: {error}");
    }
}

fn monorepo_with_unrelated_projects() -> MemoryFs {
    MemoryFs::new()
        .with_file(
            "/repo/package.json",
            r#"{"workspaces": ["x/dist", "y/dist", "!y/dist"]}"#,
        )
        .with_file("/repo/x/dist/package.json", r#"{"name": "x"}"#)
        .with_file("/repo/y/dist/package.json", r#"{"name": "y"}"#)
        .with_file("/repo/projects/p/package.json", r#"{"name": "p"}"#)
        .with_file("/repo/projects/p/src/index.ts", "")
        .with_file("/repo/projects/q/package.json", r#"{"workspaces": ["pkgs/*"]}"#)
        .with_file("/repo/projects/q/pkgs/m/package.json", r#"{"name": "m"}"#)
}

#[test]
fn an_ancestor_workspace_does_not_claim_a_project_it_does_not_include() {
    let fs = monorepo_with_unrelated_projects();
    for start in [
        "/repo/projects/p",
        "/repo/projects/p/src",
        "/repo/projects/p/src/index.ts",
    ] {
        assert_eq!(
            find_workspace_root(Path::new(start), &fs).unwrap(),
            Path::new("/repo/projects/p"),
            "{start}"
        );
    }
}

#[test]
fn a_project_with_its_own_workspace_is_its_own_root() {
    let fs = monorepo_with_unrelated_projects();
    for start in ["/repo/projects/q", "/repo/projects/q/pkgs/m"] {
        assert_eq!(
            find_workspace_root(Path::new(start), &fs).unwrap(),
            Path::new("/repo/projects/q"),
            "{start}"
        );
    }
}

#[test]
fn members_still_resolve_to_the_ancestor_workspace() {
    let fs = monorepo_with_unrelated_projects();
    assert_eq!(
        find_workspace_root(Path::new("/repo/x/dist"), &fs).unwrap(),
        Path::new("/repo")
    );
}

#[test]
fn a_negated_member_is_not_claimed() {
    let fs = monorepo_with_unrelated_projects();
    assert_eq!(
        find_workspace_root(Path::new("/repo/y/dist"), &fs).unwrap(),
        Path::new("/repo/y/dist")
    );
}

#[test]
fn a_manifest_above_the_repository_is_never_the_workspace() {
    let fs = MemoryFs::new()
        .with_file("/package.json", r#"{"name": "stray"}"#)
        .with_file("/repo/.git/HEAD", "ref")
        .with_file("/repo/a/package.json", r#"{"name": "a"}"#)
        .with_file("/repo/b/package.json", r#"{"name": "b"}"#);
    for start in ["/repo", "/repo/missing"] {
        let error = find_workspace_root(Path::new(start), &fs).unwrap_err();
        assert!(matches!(error, Error::NoWorkspace(_)), "{start}: {error}");
    }
    assert_eq!(
        find_workspace_root(Path::new("/repo/a"), &fs).unwrap(),
        Path::new("/repo/a")
    );
}

#[test]
fn the_repository_root_itself_can_be_the_workspace() {
    let fs = MemoryFs::new()
        .with_file("/package.json", r#"{"name": "stray"}"#)
        .with_file("/repo/.git/HEAD", "ref")
        .with_file("/repo/package.json", r#"{"workspaces": ["packages/*"]}"#)
        .with_file("/repo/packages/a/package.json", r#"{"name": "a"}"#);
    assert_eq!(
        find_workspace_root(Path::new("/repo/packages/a/src"), &fs).unwrap(),
        Path::new("/repo")
    );
}

fn conflicting_declarations(config: &str) -> MemoryFs {
    MemoryFs::new()
        .with_file("/repo/package.json", r#"{"workspaces": ["npmpkgs/*"]}"#)
        .with_file("/repo/pnpm-workspace.yaml", "packages:\n  - 'yarnpkgs/*'\n")
        .with_file("/repo/skif.toml", config)
        .with_file("/repo/npmpkgs/x/package.json", r#"{"name": "x"}"#)
        .with_file("/repo/yarnpkgs/y/package.json", r#"{"name": "y"}"#)
}

#[test]
fn an_ancestor_claims_members_with_its_configured_package_manager() {
    let npm = conflicting_declarations("[workspace]\npackage-manager = \"npm\"\n");
    assert_eq!(
        find_workspace_root(Path::new("/repo/npmpkgs/x"), &npm).unwrap(),
        Path::new("/repo")
    );
    assert_eq!(
        find_workspace_root(Path::new("/repo/yarnpkgs/y"), &npm).unwrap(),
        Path::new("/repo/yarnpkgs/y")
    );

    let pnpm = conflicting_declarations("[workspace]\npackage-manager = \"pnpm\"\n");
    assert_eq!(
        find_workspace_root(Path::new("/repo/yarnpkgs/y"), &pnpm).unwrap(),
        Path::new("/repo")
    );
}

#[test]
fn an_ancestor_that_cannot_be_discovered_still_claims_its_declared_members() {
    let fs = MemoryFs::new()
        .with_file("/repo/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n")
        .with_file("/repo/p/m/package.json", r#"{"name": "m"}"#);
    assert_eq!(
        find_workspace_root(Path::new("/repo/p/m"), &fs).unwrap(),
        Path::new("/repo")
    );
}
