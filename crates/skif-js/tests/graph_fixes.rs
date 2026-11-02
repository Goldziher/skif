//! Module graph, plugin and default behaviour on realistic and minimal workspaces.
//!
//! The three realistic fixtures (`vite-react`, `nextjs-app`, `jest-vitest-monorepo`) must produce no findings at all.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs, Severity};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

fn fixture_dir(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(case)
}

fn load(dir: &Path, base: &Path, memory: &mut MemoryFs, config: &mut Option<String>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            load(&path, base, memory, config);
            continue;
        }
        let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(&path).unwrap();
        if rel == "skif.toml" {
            *config = Some(text);
        } else {
            memory.insert(Path::new(ROOT).join(&rel), text);
        }
    }
}

/// Lints `memory` with the JS provider and the plugin table, the way the CLI does.
fn lint_memory(memory: MemoryFs, toml: &str) -> Vec<Diagnostic> {
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (a.rule.as_str(), &a.location.path, &a.message).cmp(&(b.rule.as_str(), &b.location.path, &b.message))
    });
    diagnostics
}

fn lint_fixture(case: &str) -> Vec<Diagnostic> {
    let base = fixture_dir(case);
    let mut memory = MemoryFs::new();
    let mut config = None;
    load(&base, &base, &mut memory, &mut config);
    lint_memory(memory, &config.unwrap_or_default())
}

fn lint_files(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    lint_memory(memory, toml)
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            let name = d.dependency.clone().unwrap_or_else(|| {
                let start = d.message.find('`').map_or(0, |i| i + 1);
                let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
                d.message[start..end].to_owned()
            });
            format!("{}: {name}", d.location.path.display())
        })
        .collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

const PACKAGE: (&str, &str) = ("package.json", r#"{"name":"app","private":true}"#);

#[test]
fn vite_react_app_has_no_findings() {
    let diagnostics = lint_fixture("vite-react");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn nextjs_app_router_has_no_findings() {
    let diagnostics = lint_fixture("nextjs-app");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn jest_and_vitest_monorepo_has_no_findings() {
    let diagnostics = lint_fixture("jest-vitest-monorepo");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn plugin_config_files_are_entries_and_their_imports_count() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"scripts":{"lint":"eslint ."},"devDependencies":{"vite":"5","@vitejs/plugin-react":"4","eslint":"9","globals":"15"}}"#,
            ),
            (
                "vite.config.ts",
                "import react from '@vitejs/plugin-react';\nimport { defineConfig } from 'vite';\nimport { helper } from './config/helper';\nexport default defineConfig({ plugins: [react(), helper()] });\n",
            ),
            ("config/helper.ts", "export const helper = () => undefined;\n"),
            (
                "eslint.config.js",
                "import globals from 'globals';\nexport default [{ languageOptions: { globals: globals.node } }];\n",
            ),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
}

#[test]
fn ambient_declaration_files_are_entries() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            ("types/global.d.ts", "declare const VERSION: string;\n"),
            ("src/index.ts", "export {};\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn conventional_test_directories_are_entries() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            ("src/index.ts", "export {};\n"),
            ("test/a.ts", "export const a = 1;\n"),
            ("tests/b.ts", "export const b = 1;\n"),
            ("e2e/c.ts", "export const c = 1;\n"),
            ("cypress/d.ts", "export const d = 1;\n"),
            ("scripts/orphan.ts", "export const orphan = 1;\n"),
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["scripts/orphan.ts: scripts/orphan.ts"]
    );
}

#[test]
fn storybook_stories_are_entries_without_a_storybook_config() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"devDependencies":{"@storybook/react":"8"}}"#,
            ),
            ("src/Button.stories.tsx", "export default {};\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn gitignore_hides_files_and_a_build_directory_is_source_otherwise() {
    let files = [
        PACKAGE,
        (
            "src/index.ts",
            "import { tool } from '../build/tool';\nexport const run = tool;\n",
        ),
        ("build/tool.ts", "export const tool = 1;\n"),
        ("generated/orphan.ts", "export const orphan = 1;\n"),
        ("orphan.ts", "export const alone = 1;\n"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["generated/orphan.ts: generated/orphan.ts", "orphan.ts: orphan.ts"]
    );

    let mut ignored = files.to_vec();
    ignored.push((".gitignore", "generated/\n"));
    let diagnostics = lint_files(&ignored, "");
    assert_eq!(found(&diagnostics, "unused-files"), ["orphan.ts: orphan.ts"]);
}

#[test]
fn default_severities_follow_the_documented_split() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"scripts":{"x":"missing-tool"},"dependencies":{"left":"1"},"devDependencies":{"dev-left":"1"}}"#,
            ),
            (
                "src/index.ts",
                "import './gone';\nimport 'unlisted';\nexport const used = 1;\n",
            ),
            ("src/orphan.ts", "export const alone = 1;\n"),
        ],
        "",
    );
    let severity = |rule: &str| {
        diagnostics
            .iter()
            .find(|d| d.rule.as_str() == rule)
            .unwrap_or_else(|| panic!("no {rule}: {diagnostics:#?}"))
            .severity
    };
    for rule in ["unused-files", "unlisted-binaries", "unused-dev-dependencies"] {
        assert_eq!(severity(rule), Severity::Warning, "{rule}");
    }
    for rule in ["unused-dependencies", "unlisted-dependencies", "unresolved-imports"] {
        assert_eq!(severity(rule), Severity::Error, "{rule}");
    }
}

#[test]
fn unresolvable_aliases_are_unresolved_imports_never_unlisted_dependencies() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"devDependencies":{"typescript":"5"}}"#,
            ),
            (
                "tsconfig.json",
                r#"{"extends":"./.generated/tsconfig.json","compilerOptions":{"baseUrl":".","paths":{"@app/*":["src/lib/*"],"@gone/*":["nowhere/*"]}}}"#,
            ),
            (
                "src/index.ts",
                "import { a } from '@app/a';\nimport { b } from '@gone/b';\nimport { c } from '@/c';\nimport { d } from '~/d';\nimport { e } from '$lib/e';\nimport '#internal/f';\nexport const all = [a, b, c, d, e];\n",
            ),
            ("src/lib/a.ts", "export const a = 1;\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        [
            "src/index.ts: #internal/f",
            "src/index.ts: $lib/e",
            "src/index.ts: @/c",
            "src/index.ts: @gone/b",
            "src/index.ts: ~/d"
        ]
    );
}

#[test]
fn bundler_aliases_computed_in_config_are_followed_or_ignored_but_never_packages() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"devDependencies":{"vite":"5"}}"#,
            ),
            (
                "vite.config.ts",
                "import { defineConfig } from 'vite';\nexport default defineConfig({ resolve: { alias: [{ find: '@app', replacement: './src/app' }, { find: '@dyn', replacement: fileURLToPath(new URL('./src', import.meta.url)) }] } });\n",
            ),
            ("index.html", "<script type=\"module\" src=\"/src/main.ts\"></script>"),
            (
                "src/main.ts",
                "import { a } from '@app/a';\nimport { b } from '@dyn/b';\nexport const all = [a, b];\n",
            ),
            ("src/app/a.ts", "export const a = 1;\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn vite_index_html_is_an_entry_without_a_vite_config() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#,
            ),
            ("index.html", "<script type=\"module\" src=\"/src/entry.ts\"></script>"),
            ("src/entry.ts", "import './side';\n"),
            ("src/side.ts", "export {};\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn workspace_override_keys_may_be_globs() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "packages/a/package.json",
            r#"{"name":"@x/a","dependencies":{"left":"1"}}"#,
        ),
        (
            "packages/b/package.json",
            r#"{"name":"@x/b","dependencies":{"left":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "export {};\n"),
        ("packages/b/src/index.ts", "export {};\n"),
    ];
    let plain = lint_files(&files, "");
    assert_eq!(found(&plain, "unused-dependencies").len(), 2);
    let globbed = lint_files(
        &files,
        "[js.workspaces.\"packages/*\"]\nignore-dependencies = [\"left\"]",
    );
    assert_eq!(found(&globbed, "unused-dependencies"), none());
}

#[test]
fn out_dir_and_root_dir_map_a_sibling_build_back_to_source() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"typescript":"5"}}"#,
            ),
            (
                "packages/a/package.json",
                r#"{"name":"@x/a","private":true,"dependencies":{"@x/b":"1"}}"#,
            ),
            (
                "packages/a/src/index.ts",
                "import { b } from '@x/b';\nexport const a = b;\n",
            ),
            (
                "packages/b/package.json",
                r#"{"name":"@x/b","version":"1.0.0","main":"./build/index.js"}"#,
            ),
            (
                "packages/b/tsconfig.json",
                r#"{"compilerOptions":{"outDir":"build","rootDir":"lib"}}"#,
            ),
            ("packages/b/lib/index.ts", "export const b = 1;\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
    assert_eq!(found(&diagnostics, "unused-exports"), none());
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn binaries_map_to_packages_that_are_not_installed() {
    let diagnostics = lint_files(
        &[(
            "package.json",
            r#"{"name":"app","private":true,"scripts":{"docs":"docusaurus build","cli":"nest start","all":"pnpm -r build","each":"yarn workspaces foreach run build","hook":"jq . x.json"},"devDependencies":{"@docusaurus/core":"3","@nestjs/cli":"10"}}"#,
        )],
        "",
    );
    assert_eq!(found(&diagnostics, "unlisted-binaries"), none());
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
}

#[test]
fn wide_barrels_attribute_usage_to_the_defining_module() {
    let count = 150;
    let mut memory = MemoryFs::new().with_file("/ws/package.json", r#"{"name":"app","private":true}"#);
    let mut barrel = String::new();
    for index in 0..count {
        let _ = writeln!(barrel, "export * from './m{index}';");
        memory.insert(
            format!("/ws/src/m{index}.ts"),
            format!("export const v{index} = {index};\n"),
        );
    }
    memory.insert("/ws/src/barrel.ts", barrel);
    memory.insert(
        "/ws/src/index.ts",
        format!(
            "import {{ v7, v{} }} from './barrel';\nexport const all = [v7, v{}];\n",
            count - 1,
            count - 1
        ),
    );
    let diagnostics = lint_memory(memory, "");
    let unused = found(&diagnostics, "unused-exports");
    assert_eq!(unused.len(), count - 2, "{unused:?}");
    assert!(!unused.iter().any(|name| name.ends_with(": v7")));
    assert!(unused.iter().any(|name| name == "src/m0.ts: v0"));
}

#[test]
fn bindings_used_only_in_component_templates_keep_their_members() {
    let files = [
        PACKAGE,
        (
            "src/color.ts",
            "export enum Color { Red, Green }\nexport const unused = 1;\n",
        ),
        (
            "src/App.vue",
            "<script setup lang=\"ts\">\nimport { Color } from './color';\n</script>\n<template><div :class=\"Color.Red\" /></template>\n",
        ),
        ("src/index.ts", "import './App.vue';\n"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unused-enum-members"), none(), "{diagnostics:#?}");

    let mut control = files.to_vec();
    control[2] = (
        "src/App.vue",
        "<script setup lang=\"ts\">\nimport { Color } from './color';\n</script>\n<template><div /></template>\n",
    );
    let diagnostics = lint_files(&control, "");
    assert!(
        !found(&diagnostics, "unused-enum-members").is_empty(),
        "{diagnostics:#?}"
    );
}

#[test]
fn url_and_mock_specifiers_keep_modules_reachable() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            (
                "src/index.ts",
                "export const worker = new URL('./worker.ts', import.meta.url);\n",
            ),
            ("src/worker.ts", "export {};\n"),
            ("src/thing.test.ts", "vi.mock('./dep');\n"),
            ("src/dep.ts", "export const dep = 1;\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn unparseable_files_are_reported_instead_of_silently_empty() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            ("src/index.ts", "import './broken';\n"),
            ("src/broken.ts", "const = ;;; )(\n"),
        ],
        "",
    );
    let found = found(&diagnostics, "unparseable-files");
    assert_eq!(found, ["src/broken.ts: src/broken.ts"], "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.rule.as_str() != "unparseable-files" || d.severity == Severity::Warning)
    );
}

const STYLE_PACKAGE: (&str, &str) = (
    "package.json",
    r#"{"name":"app","private":true,"main":"src/main.tsx","dependencies":{"tailwindcss":"4","tw-animate-css":"1","@fontsource-variable/geist":"5","sass-pkg":"1","modules-pkg":"1"},"devDependencies":{"shadcn":"1"}}"#,
);

#[test]
fn stylesheet_imports_count_as_dependency_usage() {
    let diagnostics = lint_files(
        &[
            STYLE_PACKAGE,
            ("src/main.tsx", "import './index.css';\nexport {};\n"),
            (
                "src/index.css",
                "@import \"tailwindcss\";\n@import \"tw-animate-css\";\n@import \"shadcn/tailwind.css\";\n@import \"@fontsource-variable/geist\";\n@source \"../node_modules/ignored\";\n",
            ),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: modules-pkg", "package.json: sass-pkg"]);
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
}

#[test]
fn stylesheet_chains_scss_modules_and_commented_imports() {
    let diagnostics = lint_files(
        &[
            STYLE_PACKAGE,
            (
                "src/main.tsx",
                "import './chain.css';\nimport styles from './box.module.css';\nimport './theme.scss';\nexport const s = styles;\n",
            ),
            (
                "src/chain.css",
                "@import url(\"./second.css\");\n/* @import \"missing-package\"; */\n.a { background: url(\"./not-there.png\"); }\n@import \"tailwindcss\";\n@import \"tw-animate-css\";\n@import \"shadcn/tailwind.css\";\n@import \"@fontsource-variable/geist\";\n",
            ),
            ("src/second.css", "@import './third.css' layer(base);\n"),
            ("src/third.css", ".t { color: red; }\n"),
            ("src/box.module.css", ".a { composes: b from \"./base.module.css\"; }\n"),
            ("src/base.module.css", ".b { color: red; }\n.c { composes: d from \"modules-pkg/x.css\"; }\n"),
            (
                "src/theme.scss",
                "@use \"sass:math\";\n@use \"sass-pkg\";\n@use \"partial\";\n",
            ),
            ("src/_partial.scss", "$x: 1;\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), none());
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
}

#[test]
fn unresolvable_stylesheet_imports_are_reported_but_assets_are_not() {
    let diagnostics = lint_files(
        &[
            STYLE_PACKAGE,
            ("src/main.tsx", "import './a.css';\nexport {};\n"),
            ("src/a.css", "@import \"./gone.css\";\n.x { background: url(\"./gone.png\"); }\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unresolved-imports"), ["src/a.css: ./gone.css"]);
}
