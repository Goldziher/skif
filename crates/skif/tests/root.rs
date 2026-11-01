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
