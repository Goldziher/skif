//! `[[overrides]] js`: per-package JavaScript inputs, folded after `[js.workspaces]`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::{Ceiling, ConfigCache, ConfigLoader};
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const MANIFEST: (&str, &str) = (
    "package.json",
    r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
);

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(
        MemoryFs::new().with_file("/ws/skif.toml", toml),
        |memory, (path, text)| memory.with_file(Path::new(ROOT).join(path), text),
    );
    let config = ConfigLoader::new(&memory, &ConfigCache::new())
        .resolve(Path::new(ROOT), &Ceiling::RootOnly)
        .unwrap()
        .config;
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = PluginSource::new(config.js.plugins.clone()).with_overrides(&config.overrides);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// The paths of the diagnostics of `rule`, sorted.
fn paths(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| d.location.path.display().to_string().replace('\\', "/"))
        .collect();
    found.sort();
    found
}

fn unused_dependency_files() -> Vec<(&'static str, &'static str)> {
    vec![
        MANIFEST,
        (
            "packages/a/package.json",
            r#"{"name":"@x/a","dependencies":{"left":"1","right":"1"}}"#,
        ),
        (
            "packages/b/package.json",
            r#"{"name":"@x/b","dependencies":{"left":"1","right":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "export {};\n"),
        ("packages/b/src/index.ts", "export {};\n"),
    ]
}

#[test]
fn ignore_dependencies_applies_to_the_selected_package_only() {
    let files = unused_dependency_files();
    let plain = lint(&files, "");
    assert_eq!(paths(&plain, "unused-dependencies").len(), 4);

    let scoped = lint(
        &files,
        "[[overrides]]\npackages = [\"@x/a\"]\n[overrides.js]\nignore-dependencies = [\"left\", \"right\"]\n",
    );
    assert_eq!(
        paths(&scoped, "unused-dependencies"),
        ["packages/b/package.json", "packages/b/package.json"]
    );
}

#[test]
fn extend_ignore_dependencies_adds_to_what_the_workspace_table_ignores() {
    let files = unused_dependency_files();
    let found = lint(
        &files,
        "[js.workspaces.\"packages/*\"]\nignore-dependencies = [\"left\"]\n\
         [[overrides]]\npackages = [\"@x/a\"]\n[overrides.js]\nextend-ignore-dependencies = [\"right\"]\n",
    );
    assert_eq!(paths(&found, "unused-dependencies"), ["packages/b/package.json"]);
}

#[test]
fn a_plain_override_key_replaces_the_workspace_table() {
    let files = unused_dependency_files();
    let found = lint(
        &files,
        "[js.workspaces.\"packages/*\"]\nignore-dependencies = [\"left\"]\n\
         [[overrides]]\npackages = [\"@x/a\"]\n[overrides.js]\nignore-dependencies = [\"right\"]\n",
    );
    let a: Vec<_> = paths(&found, "unused-dependencies")
        .into_iter()
        .filter(|path| path.starts_with("packages/a/"))
        .collect();
    assert_eq!(a.len(), 1, "{found:#?}");
}

fn unused_file_files() -> Vec<(&'static str, &'static str)> {
    vec![
        MANIFEST,
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ("packages/b/package.json", r#"{"name":"@x/b"}"#),
        ("packages/a/src/index.ts", "export {};\n"),
        ("packages/b/src/index.ts", "export {};\n"),
        ("packages/a/src/server.ts", "export const server = 1;\n"),
        ("packages/b/src/server.ts", "export const server = 1;\n"),
    ]
}

#[test]
fn entry_from_an_override_makes_files_used_in_that_package_only() {
    let files = unused_file_files();
    assert_eq!(
        paths(&lint(&files, ""), "unused-files"),
        ["packages/a/src/server.ts", "packages/b/src/server.ts"]
    );
    let scoped = lint(
        &files,
        "[[overrides]]\npackages = [\"./packages/a\"]\n[overrides.js]\nentry = [\"src/index.ts\", \"src/server.ts\"]\n",
    );
    assert_eq!(paths(&scoped, "unused-files"), ["packages/b/src/server.ts"]);
}

#[test]
fn ignore_from_an_override_hides_files_in_that_package_only() {
    let files = unused_file_files();
    let scoped = lint(
        &files,
        "[[overrides]]\npackages = [\"@x/b\"]\n[overrides.js]\nignore = [\"src/server.ts\"]\n",
    );
    assert_eq!(paths(&scoped, "unused-files"), ["packages/a/src/server.ts"]);
}

#[test]
fn an_override_that_selects_no_package_changes_nothing() {
    let files = unused_file_files();
    let scoped = lint(
        &files,
        "[[overrides]]\npackages = [\"nobody\"]\n[overrides.js]\nentry = [\"src/server.ts\"]\n",
    );
    assert_eq!(paths(&scoped, "unused-files"), paths(&lint(&files, ""), "unused-files"));
}
