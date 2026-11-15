//! Heft rigs in a Rush repository (ADR 0037): a project's `config/rig.json` needs the rig package, and a rig's
//! profiles are its own configs, so what their `extends` and `pluginPackage` name are its dependencies.

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

/// `<path>: <dependency>` of every diagnostic of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display().to_string().replace('\\', "/"),
                d.dependency.clone().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

const RUSH: &str = r#"{
  "rushVersion": "5.150.0",
  "pnpmVersion": "9.0.0",
  "projects": [
    { "packageName": "node-rig", "projectFolder": "rigs/node-rig" },
    { "packageName": "@acme/web-rig", "projectFolder": "rigs/web-rig" },
    { "packageName": "app-rig", "projectFolder": "rigs/app-rig" },
    { "packageName": "@acme/lib", "projectFolder": "packages/lib" }
  ]
}"#;

const NODE_RIG_HEFT: &str = r#"/**
 * The build of the projects that use this rig.
 */
{
  "$schema": "https://developer.microsoft.com/json-schemas/heft/v0/heft.schema.json",
  "extends": "@rushstack/heft-node-rig/profiles/default/config/heft.json",
  "heftPlugins": [{ "pluginPackage": "@acme/heft-metrics-plugin" }],
  "phasesByName": {
    "build": {
      "tasksByName": {
        "lint": { "taskPlugin": { "pluginPackage": "@rushstack/heft-lint-plugin" } }
      }
    }
  }
}"#;

const SASS_HEFT: &str = r#"{
  "phasesByName": {
    "build": {
      "tasksByName": {
        "sass": { "taskPlugin": { "pluginPackage": "@rushstack/heft-sass-plugin" } }
      }
    }
  }
}"#;

/// The files of the repository: `node-rig` extends a published rig and names a task and a lifecycle plugin, its
/// profile `tsconfig` sets `importHelpers` without the rig listing `tslib`; `@acme/web-rig` and `app-rig` run the Sass
/// plugin, only the first listing `sass`; `@acme/lib` uses `node-rig` through `config/rig.json` when `rig_json`.
fn repository(rig_json: bool) -> Vec<(&'static str, &'static str)> {
    let mut files = vec![
        ("rush.json", RUSH),
        // Like published rigs, each rig ships a shared ESLint profile, so it has code of its own.
        (
            "rigs/node-rig/profiles/default/includes/eslint/profile.js",
            "module.exports = {};\n",
        ),
        (
            "rigs/web-rig/profiles/library/includes/eslint/profile.js",
            "module.exports = {};\n",
        ),
        (
            "rigs/app-rig/profiles/default/includes/eslint/profile.js",
            "module.exports = {};\n",
        ),
        (
            "rigs/node-rig/package.json",
            r#"{"name":"node-rig","version":"1.0.0","private":true,"dependencies":{
              "@rushstack/heft":"1.3.4","@rushstack/heft-node-rig":"2.11.55","@rushstack/heft-lint-plugin":"1.0.0",
              "@acme/heft-metrics-plugin":"1.0.0","@types/node":"20.17.19","left-pad":"1.3.0"}}"#,
        ),
        ("rigs/node-rig/profiles/default/config/heft.json", NODE_RIG_HEFT),
        (
            "rigs/node-rig/profiles/default/config/jest.config.json",
            r#"{"extends":"@rushstack/heft-node-rig/profiles/default/config/jest.config.json"}"#,
        ),
        (
            "rigs/node-rig/profiles/default/tsconfig-base.json",
            r#"{"compilerOptions":{"importHelpers":true,"types":["node"],"outDir":"../../../../lib"}}"#,
        ),
        (
            "rigs/web-rig/package.json",
            r#"{"name":"@acme/web-rig","version":"1.0.0","dependencies":{"@rushstack/heft-sass-plugin":"1.0.0",
              "sass":"1.49.7","tslib":"2.8.1"},"devDependencies":{"@rushstack/heft":"1.3.4"}}"#,
        ),
        ("rigs/web-rig/profiles/library/config/heft.json", SASS_HEFT),
        (
            "rigs/web-rig/profiles/library/tsconfig-base.json",
            r#"{"compilerOptions":{"importHelpers":true}}"#,
        ),
        (
            "rigs/app-rig/package.json",
            r#"{"name":"app-rig","version":"1.0.0","private":true,"dependencies":{"@rushstack/heft":"1.3.4",
              "@rushstack/heft-sass-plugin":"1.0.0"}}"#,
        ),
        ("rigs/app-rig/profiles/default/config/heft.json", SASS_HEFT),
        (
            "packages/lib/package.json",
            r#"{"name":"@acme/lib","version":"1.0.0","main":"src/index.ts","devDependencies":{"@rushstack/heft-node-rig":"2.11.55"}}"#,
        ),
        ("packages/lib/src/index.ts", "export const a = 1;\n"),
    ];
    if rig_json {
        files.push((
            "packages/lib/config/rig.json",
            r#"{
              // Tools find their config files in the rig.
              "$schema": "https://developer.microsoft.com/json-schemas/rig-package/rig.schema.json",
              "rigPackageName": "@rushstack/heft-node-rig"
            }"#,
        ));
    }
    files
}

/// What a rig's profiles name is used, `sass` through the Sass plugin, and `tslib` that a profile `tsconfig`'s
/// `importHelpers` needs is no unlisted dependency of the rig, which builds nothing itself. A dependency nothing names
/// is still unused, and the project that names the rig in `config/rig.json` uses it.
#[test]
fn rig_profiles_credit_the_rigs_dependencies_and_rig_json_the_rig() {
    let diagnostics = lint(&repository(true));
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["rigs/node-rig/package.json: left-pad"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// Without `config/rig.json` nothing names the rig in the project, which then lists it without using it.
#[test]
fn a_rig_no_rig_json_names_is_unused() {
    let diagnostics = lint(&repository(false));
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["packages/lib/package.json: @rushstack/heft-node-rig"],
        "{diagnostics:#?}"
    );
}

/// A Heft project: its Jest task runs the compiled tests and its Storybook task the storykit and CLI packages its
/// options name.
fn heft_project() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "rush.json",
            r#"{"rushVersion":"5.150.0","pnpmVersion":"9.0.0","projects":[{"packageName":"app","projectFolder":"app"}]}"#,
        ),
        (
            "app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"./src/index.ts","scripts":{"build":"heft build"},"devDependencies":{
              "@rushstack/heft":"1.3.4","@rushstack/heft-jest-plugin":"1.0.0","@rushstack/heft-storybook-plugin":"1.0.0",
              "app-storykit":"1.0.0","@storybook/react":"6.4.22"}}"#,
        ),
        (
            "app/config/heft.json",
            r#"{
  "phasesByName": {
    "build": {
      "tasksByName": {
        "storybook": {
          "taskPlugin": {
            "pluginPackage": "@rushstack/heft-storybook-plugin",
            "options": { "storykitPackageName": "app-storykit", "cliPackageName": "@storybook/react" }
          }
        }
      }
    },
    "test": {
      "tasksByName": { "jest": { "taskPlugin": { "pluginPackage": "@rushstack/heft-jest-plugin" } } }
    }
  }
}"#,
        ),
        ("app/src/index.ts", "export const a = 1;\n"),
        (
            "app/src/test/index.test.ts",
            "import { helper } from './helper';\nhelper();\n",
        ),
        ("app/src/test/helper.ts", "export function helper(): void {}\n"),
        ("app/src/orphan.ts", "export const b = 2;\n"),
    ]
}

/// The tests of the Jest task are entries and the packages the Storybook task's options name are used; only the file
/// nothing reaches is unused.
#[test]
fn heft_tasks_credit_their_tests_and_option_packages() {
    let diagnostics = lint(&heft_project());
    assert_eq!(found(&diagnostics, "unused-files"), ["app/src/orphan.ts: "]);
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), Vec::<String>::new());
    assert_eq!(found(&diagnostics, "unused-dependencies"), Vec::<String>::new());
}

/// The Storybook plugin resolves its storykit from the project and the CLI package from the storykit: a project that
/// does not list its storykit lacks it, while the CLI package is the storykit's dependency, not the project's.
#[test]
fn the_storybook_cli_package_is_a_dependency_of_the_storykit() {
    let diagnostics = lint(&[
        (
            "rush.json",
            r#"{"rushVersion":"5.150.0","pnpmVersion":"9.0.0","projects":[
              {"packageName":"app","projectFolder":"app"},{"packageName":"app-storykit","projectFolder":"storykit"}]}"#,
        ),
        (
            "app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"./src/index.ts","scripts":{"build":"heft build --storybook"},
              "devDependencies":{"@rushstack/heft":"1.3.4","@rushstack/heft-storybook-plugin":"1.0.0"}}"#,
        ),
        (
            "app/config/heft.json",
            r#"{
  "phasesByName": {
    "build": {
      "tasksByName": {
        "storybook": {
          "taskPlugin": {
            "pluginPackage": "@rushstack/heft-storybook-plugin",
            "options": { "storykitPackageName": "app-storykit", "cliPackageName": "@storybook/react" }
          }
        }
      }
    }
  }
}"#,
        ),
        ("app/src/index.ts", "export const a = 1;\n"),
        (
            "storykit/package.json",
            r#"{"name":"app-storykit","version":"1.0.0","main":"index.js","dependencies":{"@storybook/react":"6.4.22"}}"#,
        ),
        ("storykit/index.js", "module.exports = {};\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["app/config/heft.json: app-storykit"]
    );
}
