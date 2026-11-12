//! Workspace dependencies that only order tasks: a member with no module entry and no source file (a tooling, asset
//! or style package) cannot be imported or run, so a Turborepo `^build` or Nx task dependency on it is not reported
//! as unused. A member that has either is still judged.
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

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `(rule, dependency)` of every unused-dependency finding, sorted.
fn unused(diagnostics: &[Diagnostic]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = diagnostics
        .iter()
        .filter(|d| matches!(d.rule.as_str(), "unused-dependencies" | "unused-dev-dependencies"))
        .filter_map(|d| Some((d.rule.as_str().to_owned(), d.dependency.clone()?)))
        .collect();
    found.sort();
    found
}

const ROOT_MANIFEST: &str = r#"{"name":"root","private":true,"workspaces":["apps/*","packages/*"]}"#;

const APP: &str = r#"{
  "name": "app",
  "private": true,
  "main": "index.js",
  "scripts": { "build": "echo building app" },
  "dependencies": {
    "tooling-config": "workspace:*",
    "published": "workspace:*",
    "util": "workspace:*"
  },
  "devDependencies": { "shared-assets": "workspace:*", "shared-styles": "workspace:*" }
}"#;

#[test]
fn dependencies_on_members_without_a_module_entry_or_source_are_not_reported() {
    let diagnostics = lint(&[
        ("package.json", ROOT_MANIFEST),
        ("apps/app/package.json", APP),
        ("apps/app/index.js", "export const app = 1;\n"),
        (
            "packages/tooling-config/package.json",
            r#"{"name":"tooling-config","private":true,"scripts":{"build":"echo building tooling-config"}}"#,
        ),
        ("packages/shared-assets/package.json", r#"{"name":"shared-assets","private":true}"#),
        ("packages/shared-assets/logo.svg", "<svg/>"),
        ("packages/shared-styles/package.json", r#"{"name":"shared-styles","private":true}"#),
        ("packages/shared-styles/theme.scss", "$primary: red;\n"),
        // A module entry with no source: something may import its build output.
        (
            "packages/published/package.json",
            r#"{"name":"published","private":true,"main":"dist/index.js"}"#,
        ),
        // A source file without a module entry: Node resolves `util` to its `index.js`.
        ("packages/util/package.json", r#"{"name":"util","private":true}"#),
        ("packages/util/index.js", "export const util = 1;\n"),
    ]);
    assert_eq!(
        unused(&diagnostics),
        [
            ("unused-dependencies".to_owned(), "published".to_owned()),
            ("unused-dependencies".to_owned(), "util".to_owned()),
        ]
    );
}

#[test]
fn every_module_entry_field_makes_a_member_importable() {
    for field in [
        r#""main":"index.js""#,
        r#""module":"index.mjs""#,
        r#""exports":"./index.js""#,
        r#""bin":"cli.js""#,
        r#""browser":"index.js""#,
        r#""types":"index.d.ts""#,
        r#""typings":"index.d.ts""#,
    ] {
        let target = format!(r#"{{"name":"target","private":true,{field}}}"#);
        let diagnostics = lint(&[
            ("package.json", ROOT_MANIFEST),
            (
                "apps/app/package.json",
                r#"{"name":"app","private":true,"main":"index.js","dependencies":{"target":"workspace:*"}}"#,
            ),
            ("apps/app/index.js", "export const app = 1;\n"),
            ("packages/target/package.json", &target),
        ]);
        assert_eq!(
            unused(&diagnostics),
            [("unused-dependencies".to_owned(), "target".to_owned())],
            "{field}"
        );
    }
}

#[test]
fn a_dependency_that_is_no_member_is_still_judged() {
    let diagnostics = lint(&[
        ("package.json", ROOT_MANIFEST),
        (
            "apps/app/package.json",
            r#"{"name":"app","private":true,"main":"index.js","dependencies":{"left-pad":"1.3.0"}}"#,
        ),
        ("apps/app/index.js", "export const app = 1;\n"),
    ]);
    assert_eq!(
        unused(&diagnostics),
        [("unused-dependencies".to_owned(), "left-pad".to_owned())]
    );
}
