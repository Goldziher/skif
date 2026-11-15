//! `paths` mappings named like a dependency, as TypeScript resolves them (ADR 0013): the `paths` of a `tsconfig.json`
//! and of `[js.workspaces.<dir>.paths]` win over an installed package or a workspace member of the same name, and an
//! import they lead into the package's own sources uses no listed dependency. A mapping whose targets are missing
//! falls through to the package. The `paths` of `[js.workspaces.<dir>.paths]` also win over the `paths` of the
//! workspace's `tsconfig.json` or `jsconfig.json`.
//!
//! Every case is a minimal synthetic workspace: `app` lists the workspace member `src` or the installed package
//! `lib` and maps a specifier starting with that name into its own sources, or maps `*` to its own sources and to a
//! sibling directory in the opposite order of its compiler config.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };
const ROOT_MANIFEST: &str = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;

/// The rules an import named like a dependency can change.
const RULES: [&str; 4] = [
    "unlisted-dependencies",
    "unresolved-imports",
    "unused-dependencies",
    "unused-files",
];

fn provider_run(workspace: &Workspace, config: &Config, fs: Arc<dyn FileSystem>) -> Vec<Diagnostic> {
    let source = PluginSource::new(config.js.plugins.clone()).with_workspaces(&config.js.workspaces);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, workspace, config, &RunOptions::default()).unwrap()
}

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    provider_run(&workspace, &config, Arc::new(memory))
}

/// [`lint`] on disk, with `node_modules/src` linked to the member as a package manager installs it.
#[cfg(unix)]
fn lint_installed(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::create_dir_all(root.join("node_modules")).unwrap();
    std::os::unix::fs::symlink("../packages/src", root.join("node_modules/src")).unwrap();
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover(&root, &config).unwrap();
    provider_run(&workspace, &config, Arc::new(skif_core::OsFs))
}

/// `<rule> <path>: <dependency>` of every diagnostic of [`RULES`], sorted.
fn findings(diagnostics: &[Diagnostic]) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .iter()
        .filter(|d| RULES.contains(&d.rule.as_str()))
        .map(|d| {
            format!(
                "{} {}: {}",
                d.rule.as_str(),
                d.location.path.display(),
                d.dependency.clone().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

fn none() -> Vec<String> {
    Vec::new()
}

/// `app` lists the member `src` and imports `src/Feature/X`, which it has (`local`) or not; the member has the same
/// file.
#[cfg(unix)]
fn member_tree(app_tsconfig: Option<&'static str>, local: bool) -> Vec<(&'static str, &'static str)> {
    let mut files = vec![
        ("package.json", ROOT_MANIFEST),
        (
            "packages/src/package.json",
            r#"{"name":"src","version":"1.0.0","main":"index.ts","exports":{".":"./index.ts","./*":"./*.ts"}}"#,
        ),
        ("packages/src/index.ts", "export const index = 1;\n"),
        ("packages/src/Feature/X.ts", "export const X = \"member\";\n"),
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","dependencies":{"src":"workspace:*"}}"#,
        ),
        (
            "packages/app/src/index.ts",
            "import { X } from 'src/Feature/X';\nconsole.log(X);\n",
        ),
    ];
    if let Some(tsconfig) = app_tsconfig {
        files.push(("packages/app/tsconfig.json", tsconfig));
    }
    if local {
        files.push(("packages/app/src/Feature/X.ts", "export const X = \"local\";\n"));
    }
    files
}

/// `app` lists the installed `lib` and imports `lib/util/helper`; `src/lib/util/helper.ts` is there when `local`.
fn installed_tree(app_tsconfig: Option<&'static str>, local: bool) -> Vec<(&'static str, &'static str)> {
    let mut files = vec![
        ("package.json", ROOT_MANIFEST),
        (
            "node_modules/lib/package.json",
            r#"{"name":"lib","version":"1.0.0","main":"index.js"}"#,
        ),
        ("node_modules/lib/index.js", "module.exports = {};\n"),
        (
            "node_modules/lib/util/helper.js",
            "exports.helper = () => \"installed\";\n",
        ),
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","dependencies":{"lib":"1.0.0"}}"#,
        ),
        (
            "packages/app/src/index.ts",
            "import { helper } from 'lib/util/helper';\nconsole.log(helper);\n",
        ),
    ];
    if let Some(tsconfig) = app_tsconfig {
        files.push(("packages/app/tsconfig.json", tsconfig));
    }
    if local {
        files.push((
            "packages/app/src/lib/util/helper.ts",
            "export const helper = () => \"local\";\n",
        ));
    }
    files
}

#[cfg(unix)]
const SRC_TSCONFIG: &str = r#"{"compilerOptions":{"baseUrl":".","paths":{"src/*":["./src/*"]}}}"#;
#[cfg(unix)]
const SRC_PATHS: &str = "[js.workspaces.\"packages/app\".paths]\n\"src/*\" = [\"./src/*\"]\n";
const LIB_TSCONFIG: &str = r#"{"compilerOptions":{"baseUrl":".","paths":{"lib/*":["./src/lib/*"]}}}"#;
const LIB_PATHS: &str = "[js.workspaces.\"packages/app\".paths]\n\"lib/*\" = [\"./src/lib/*\"]\n";

#[cfg(unix)]
#[test]
fn tsconfig_paths_into_own_sources_do_not_use_the_member_named_like_the_prefix() {
    let diagnostics = lint_installed(&member_tree(Some(SRC_TSCONFIG), true), "");
    assert_eq!(
        findings(&diagnostics),
        ["unused-dependencies packages/app/package.json: src"],
        "{diagnostics:#?}"
    );
}

#[test]
fn tsconfig_paths_into_own_sources_do_not_use_the_installed_package_named_like_the_prefix() {
    let diagnostics = lint(&installed_tree(Some(LIB_TSCONFIG), true), "");
    assert_eq!(
        findings(&diagnostics),
        ["unused-dependencies packages/app/package.json: lib"],
        "{diagnostics:#?}"
    );
}

#[cfg(unix)]
#[test]
fn configured_paths_win_over_the_member_named_like_the_prefix() {
    let diagnostics = lint_installed(&member_tree(None, true), SRC_PATHS);
    assert_eq!(
        findings(&diagnostics),
        ["unused-dependencies packages/app/package.json: src"],
        "the local file is reached, not the member's: {diagnostics:#?}"
    );
}

#[test]
fn configured_paths_win_over_the_installed_package_named_like_the_prefix() {
    let diagnostics = lint(&installed_tree(None, true), LIB_PATHS);
    assert_eq!(
        findings(&diagnostics),
        ["unused-dependencies packages/app/package.json: lib"],
        "the local file is reached, not the installed one: {diagnostics:#?}"
    );
}

#[cfg(unix)]
#[test]
fn configured_paths_whose_target_is_missing_fall_through_to_the_member() {
    let diagnostics = lint_installed(&member_tree(None, false), SRC_PATHS);
    assert_eq!(findings(&diagnostics), none(), "{diagnostics:#?}");
}

#[test]
fn paths_whose_target_is_missing_fall_through_to_the_installed_package() {
    for (tsconfig, toml) in [(Some(LIB_TSCONFIG), ""), (None, LIB_PATHS)] {
        let diagnostics = lint(&installed_tree(tsconfig, false), toml);
        assert_eq!(findings(&diagnostics), none(), "{tsconfig:?} {toml}: {diagnostics:#?}");
    }
}

#[test]
fn an_import_no_paths_entry_matches_still_uses_the_listed_package() {
    let other_tsconfig = r#"{"compilerOptions":{"baseUrl":".","paths":{"local/*":["./src/local/*"]}}}"#;
    let other_paths = "[js.workspaces.\"packages/app\".paths]\n\"local/*\" = [\"./src/local/*\"]\n";
    for (tsconfig, toml) in [(Some(other_tsconfig), ""), (None, other_paths)] {
        let diagnostics = lint(&installed_tree(tsconfig, true), toml);
        assert_eq!(
            findings(&diagnostics),
            ["unused-files packages/app/src/lib/util/helper.ts: "],
            "{tsconfig:?} {toml}: {diagnostics:#?}"
        );
    }
}

/// A `paths` entry that maps the package to a declaration file only types it: the import still loads the listed
/// package at runtime, so the package is used.
#[test]
fn paths_to_a_declaration_file_still_use_the_listed_package() {
    let mut files = installed_tree(
        Some(r#"{"compilerOptions":{"baseUrl":".","paths":{"lib/*":["./types/lib.d.ts"]}}}"#),
        false,
    );
    files.push((
        "packages/app/types/lib.d.ts",
        "export declare function helper(): string;\n",
    ));
    let diagnostics = lint(&files, "");
    assert_eq!(findings(&diagnostics), none(), "{diagnostics:#?}");
}

/// `app` imports `svc`, which `app/src/svc` and `shared/svc` both provide; `config` (`tsconfig.json` or
/// `jsconfig.json`) maps `*` to `shared` first.
fn shadowed_tree(config: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", r#"{"name":"root","private":true,"workspaces":["app"]}"#),
        (
            "app/package.json",
            r#"{"name":"app","private":true,"type":"module","main":"src/main.js"}"#,
        ),
        (
            config,
            r#"{"compilerOptions":{"paths":{"*":["../shared/*","./src/*"]}}}"#,
        ),
        ("app/src/main.js", "import svc from 'svc';\nconsole.log(svc);\n"),
        ("app/src/svc/index.js", "export default \"app\";\n"),
        ("shared/svc/index.js", "export default \"shared\";\n"),
    ]
}

/// The configured `paths` come first whether the compiler options are in a `tsconfig.json` or a `jsconfig.json`:
/// the import reaches `app/src/svc`, and the `shared` copy the compiler config tries first is unused.
#[test]
fn configured_paths_win_over_the_paths_of_the_compiler_config() {
    let toml = "[js.workspaces.app.paths]\n\"*\" = [\"./src/*\", \"../shared/*\"]\n";
    for config in ["app/tsconfig.json", "app/jsconfig.json"] {
        let diagnostics = lint(&shadowed_tree(config), toml);
        assert_eq!(
            findings(&diagnostics),
            ["unused-files shared/svc/index.js: "],
            "{config}: {diagnostics:#?}"
        );
    }
}
