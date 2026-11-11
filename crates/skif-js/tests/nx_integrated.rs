//! An integrated Nx workspace (ADR 0109): projects with a `project.json` and no `package.json`, whose dependencies the
//! root lists. Their tool configs are read, so neither the configs, the files they make entries nor the packages they
//! name are reported, while an unused file of a project and an unused dependency still are.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
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

fn unused_files(diagnostics: &[Diagnostic]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "unused-files")
        .map(|d| d.location.path.clone())
        .collect();
    paths.sort();
    paths
}

fn dependencies(diagnostics: &[Diagnostic], rules: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = diagnostics
        .iter()
        .filter(|d| rules.contains(&d.rule.as_str()))
        .filter_map(|d| d.dependency.clone())
        .collect();
    names.sort();
    names
}

const MANIFEST: &str = r#"{
  "name": "root",
  "private": true,
  "dependencies": { "react": "19.0.0", "left-pad": "1.0.0" },
  "devDependencies": {
    "nx": "20.0.0",
    "vite": "6.0.0",
    "@vitejs/plugin-react": "4.0.0",
    "eslint": "9.0.0",
    "eslint-plugin-react": "7.0.0",
    "jest": "29.0.0",
    "ts-jest": "29.0.0"
  }
}"#;

fn workspace_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", MANIFEST),
        ("nx.json", "{}"),
        ("apps/web/project.json", r#"{ "name": "web" }"#),
        (
            "apps/web/vite.config.ts",
            "import react from '@vitejs/plugin-react';\nimport { defineConfig } from 'vite';\nexport default defineConfig({ root: __dirname, plugins: [react()] });\n",
        ),
        (
            "apps/web/index.html",
            r#"<html><body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body></html>"#,
        ),
        (
            "apps/web/src/main.tsx",
            "import { createElement } from 'react';\nimport { App } from './app';\ncreateElement(App);\n",
        ),
        ("apps/web/src/app.tsx", "export function App() {\n  return null;\n}\n"),
        ("apps/web/src/unused.ts", "export const unused = 1;\n"),
        (
            "apps/web/eslint.config.mjs",
            "import react from 'eslint-plugin-react';\nexport default [{ plugins: { react } }];\n",
        ),
        ("libs/util/project.json", r#"{ "name": "util" }"#),
        (
            "libs/util/jest.config.ts",
            "export default { transform: { '^.+\\\\.ts$': 'ts-jest' } };\n",
        ),
        (
            "libs/util/src/sum.ts",
            "export const sum = (a: number, b: number) => a + b;\n",
        ),
        (
            "libs/util/src/sum.spec.ts",
            "import { sum } from './sum';\ntest('sum', () => expect(sum(1, 2)).toBe(3));\n",
        ),
    ]
}

#[test]
fn the_tool_configs_of_integrated_projects_count_for_the_root() {
    let diagnostics = lint(&workspace_files());
    assert_eq!(
        unused_files(&diagnostics),
        [PathBuf::from("apps/web/src/unused.ts")],
        "{diagnostics:#?}"
    );
    // The packages the project configs and sources use, besides one nothing uses.
    let judged = [
        "@vitejs/plugin-react",
        "eslint-plugin-react",
        "left-pad",
        "react",
        "ts-jest",
        "vite",
    ];
    let unused: Vec<String> = dependencies(&diagnostics, &["unused-dependencies", "unused-dev-dependencies"])
        .into_iter()
        .filter(|name| judged.contains(&name.as_str()))
        .collect();
    assert_eq!(unused, ["left-pad"], "{diagnostics:#?}");
}

/// The same project without `nx` among the root's dependencies is a plain directory: its configs are not read.
#[test]
fn without_nx_a_project_json_is_a_plain_file() {
    let manifest = MANIFEST.replace("\"nx\": \"20.0.0\",", "");
    assert_ne!(manifest, MANIFEST);
    let files: Vec<(&str, &str)> = workspace_files()
        .into_iter()
        .map(|(path, text)| {
            (
                path,
                if path == "package.json" {
                    manifest.as_str()
                } else {
                    text
                },
            )
        })
        .collect();
    let diagnostics = lint(&files);
    let configs: Vec<PathBuf> = unused_files(&diagnostics)
        .into_iter()
        .filter(|path| path.to_string_lossy().contains(".config."))
        .collect();
    assert_eq!(
        configs,
        [
            PathBuf::from("apps/web/eslint.config.mjs"),
            PathBuf::from("apps/web/vite.config.ts"),
            PathBuf::from("libs/util/jest.config.ts"),
        ],
        "{diagnostics:#?}"
    );
}

/// Nx's root `jest.config.ts` takes its projects from `getJestProjectsAsync()`: they are the Jest configs of the
/// projects, so the root is judged as without the file.
#[test]
fn the_nx_root_jest_config_leaves_the_root_judged() {
    let mut files = workspace_files();
    files.push((
        "jest.config.ts",
        "import { getJestProjectsAsync } from '@nx/jest';\n\nexport default async () => ({\n  projects: await getJestProjectsAsync(),\n});\n",
    ));
    let with_root_config = lint(&files);
    assert_eq!(
        unused_files(&with_root_config),
        [PathBuf::from("apps/web/src/unused.ts")],
        "{with_root_config:#?}"
    );
    let unused =
        |diagnostics: &[Diagnostic]| dependencies(diagnostics, &["unused-dependencies", "unused-dev-dependencies"]);
    assert_eq!(unused(&with_root_config), unused(&lint(&workspace_files())));
}

/// A package-based workspace: the member's own `project.json` names its paths relative to the workspace root, so its
/// build entry and the script a command runs from the root are entries of the member, and the root does not read it.
#[test]
fn a_members_project_json_names_its_files_from_the_workspace_root() {
    let project = r#"{ "targets": {
      "build": { "options": { "main": "packages/b/src/entry.ts" } },
      "gen": { "command": "node packages/b/scripts/gen.mjs" },
      "pack": { "executor": "nx:run-commands", "options": { "command": "node tools/pack.mjs", "cwd": "packages/b" } }
    } }"#;
    let files = [
        (
            "package.json",
            r#"{ "name": "root", "private": true, "workspaces": ["packages/*"], "devDependencies": { "nx": "20.0.0" } }"#,
        ),
        ("nx.json", "{}"),
        ("packages/b/package.json", r#"{ "name": "b", "version": "1.0.0" }"#),
        ("packages/b/project.json", project),
        ("packages/b/src/entry.ts", "import { helper } from './helper';\nhelper();\n"),
        ("packages/b/src/helper.ts", "export function helper() {}\n"),
        ("packages/b/src/unused.ts", "export const unused = 1;\n"),
        ("packages/b/scripts/gen.mjs", "console.log('gen');\n"),
        ("packages/b/tools/pack.mjs", "console.log('pack');\n"),
    ];
    let diagnostics = lint(&files);
    assert_eq!(
        unused_files(&diagnostics),
        [PathBuf::from("packages/b/src/unused.ts")],
        "{diagnostics:#?}"
    );
}
