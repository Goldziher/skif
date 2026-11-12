//! Imports that a package's own tsconfig `paths` map into its sources, in every mode: never an import of the
//! workspace package whose name the specifier starts with, so strict mode reports no unlisted dependency for them.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

/// The modes a run can be in: default, `--production`, and `--strict`, which implies `--production`.
const MODES: [(&str, bool, bool); 3] = [("default", false, false), ("production", true, false), ("strict", true, true)];

fn lint(files: &[(&str, &str)], production: bool, strict: bool) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let mut config = Config::from_toml_str("").unwrap();
    config.js.production = production;
    config.js.isolate_workspaces = strict;
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `<path>: <dependency>` of every diagnostic of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display(),
                d.dependency.clone().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

/// `app` maps `src/*` to its own sources without a `baseUrl`, extending a root config; another workspace package is
/// named `src`.
fn workspace_with(app_tsconfig: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        ("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#),
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"main":"src/main.ts"}"#,
        ),
        ("packages/app/tsconfig.json", app_tsconfig),
        (
            "packages/app/src/main.ts",
            "import { X } from 'src/Feature/X';\nconsole.log(X);\n",
        ),
        ("packages/app/src/Feature/X.ts", "export const X = 1;\n"),
        (
            "packages/other/package.json",
            r#"{"name":"src","version":"1.0.0","private":true,"main":"index.ts"}"#,
        ),
        ("packages/other/index.ts", "export const Y = 1;\n"),
    ]
}

const WITHOUT_BASE_URL: &str =
    r#"{"extends":"../../tsconfig.json","compilerOptions":{"paths":{"src/*":["./src/*"]}}}"#;
const WITH_BASE_URL: &str =
    r#"{"extends":"../../tsconfig.json","compilerOptions":{"baseUrl":".","paths":{"src/*":["./src/*"]}}}"#;

#[test]
fn own_paths_into_own_sources_are_never_the_member_named_like_the_prefix() {
    for tsconfig in [WITHOUT_BASE_URL, WITH_BASE_URL] {
        for (mode, production, strict) in MODES {
            let diagnostics = lint(&workspace_with(tsconfig), production, strict);
            assert_eq!(
                found(&diagnostics, "unlisted-dependencies"),
                Vec::<String>::new(),
                "{mode}, {tsconfig}: {diagnostics:#?}"
            );
            assert!(
                !found(&diagnostics, "unused-files").contains(&"packages/app/src/Feature/X.ts: ".to_owned()),
                "{mode}, {tsconfig}: the mapped file is reached: {diagnostics:#?}"
            );
        }
    }
}

#[test]
fn a_listed_member_reached_only_through_own_paths_is_unused() {
    // The mapped import is not a use of the member `src`, so listing it leaves a dependency nothing uses.
    let mut files = workspace_with(WITHOUT_BASE_URL);
    files[2] = (
        "packages/app/package.json",
        r#"{"name":"app","version":"1.0.0","private":true,"main":"src/main.ts","dependencies":{"src":"workspace:*"}}"#,
    );
    let diagnostics = lint(&files, false, false);
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["packages/app/package.json: src"],
        "{diagnostics:#?}"
    );
}
