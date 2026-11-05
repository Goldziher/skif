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
    lint_memory_at(memory, toml, ROOT)
}

/// Like `lint_memory`, with the workspace rooted at `root` (a directory inside the memory file system).
fn lint_memory_at(memory: MemoryFs, toml: &str, root: &str) -> Vec<Diagnostic> {
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(root), &config, &memory).unwrap();
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
fn rspack_jsconfig_app_has_no_false_positives() {
    let diagnostics = lint_fixture("rspack-jsconfig-app");
    for rule in [
        "unused-files",
        "unresolved-imports",
        "unlisted-dependencies",
        "unused-dependencies",
        "unused-dev-dependencies",
    ] {
        assert_eq!(found(&diagnostics, rule), none(), "{rule}: {diagnostics:#?}");
    }
}

#[test]
fn react_is_used_by_the_automatic_jsx_runtime() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"main":"src/index.jsx","dependencies":{"react":"18"}}"#,
            ),
            ("src/index.jsx", "export const App = () => <div />;\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), none());
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
fn test_files_and_directories_are_entries_only_through_a_test_runner() {
    let files = [
        ("src/index.ts", "export {};\n"),
        ("src/legacy.spec.ts", "export const legacy = 1;\n"),
        ("test/a.ts", "export const a = 1;\n"),
        ("tests/b.ts", "export const b = 1;\n"),
        ("e2e/c.ts", "export const c = 1;\n"),
        ("scripts/orphan.ts", "export const orphan = 1;\n"),
    ];
    let mut without: Vec<(&str, &str)> = vec![PACKAGE];
    without.extend(files);
    let mut unused = found(&lint_files(&without, ""), "unused-files");
    unused.sort();
    assert_eq!(
        unused,
        [
            "e2e/c.ts: e2e/c.ts",
            "scripts/orphan.ts: scripts/orphan.ts",
            "src/legacy.spec.ts: src/legacy.spec.ts",
            "test/a.ts: test/a.ts",
            "tests/b.ts: tests/b.ts"
        ]
    );

    let mut with: Vec<(&str, &str)> = vec![(
        "package.json",
        r#"{"name":"app","private":true,"devDependencies":{"vitest":"3"}}"#,
    )];
    with.extend(files);
    let mut unused = found(&lint_files(&with, ""), "unused-files");
    unused.sort();
    assert_eq!(
        unused,
        [
            "e2e/c.ts: e2e/c.ts",
            "scripts/orphan.ts: scripts/orphan.ts",
            "test/a.ts: test/a.ts",
            "tests/b.ts: tests/b.ts"
        ]
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
fn unresolvable_aliases_are_unresolved_imports_unless_they_look_like_packages() {
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
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), ["src/index.ts: @gone/b"]);
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        [
            "src/index.ts: #internal/f",
            "src/index.ts: $lib/e",
            "src/index.ts: @/c",
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
    assert_eq!(
        found(&diagnostics, "unused-enum-members"),
        ["src/color.ts: Color.Green"],
        "{diagnostics:#?}"
    );

    let mut whole = files.to_vec();
    whole[2] = (
        "src/App.vue",
        "<script setup lang=\"ts\">\nimport { Color } from './color';\n</script>\n<template><Pick :options=\"Color\" /></template>\n",
    );
    let diagnostics = lint_files(&whole, "");
    assert_eq!(found(&diagnostics, "unused-enum-members"), none(), "{diagnostics:#?}");

    let mut control = files.to_vec();
    control[2] = (
        "src/App.vue",
        "<script setup lang=\"ts\">\nimport { Color } from './color';\n</script>\n<template><div /></template>\n",
    );
    let diagnostics = lint_files(&control, "");
    assert_eq!(
        found(&diagnostics, "unused-enum-members"),
        ["src/color.ts: Color.Green", "src/color.ts: Color.Red"],
        "{diagnostics:#?}"
    );
}

#[test]
fn url_and_mock_specifiers_keep_modules_reachable() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"devDependencies":{"vitest":"3"}}"#,
            ),
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
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["package.json: modules-pkg", "package.json: sass-pkg"]
    );
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
            (
                "src/base.module.css",
                ".b { color: red; }\n.c { composes: d from \"modules-pkg/x.css\"; }\n",
            ),
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
            (
                "src/a.css",
                "@import \"./gone.css\";\n.x { background: url(\"./gone.png\"); }\n",
            ),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unresolved-imports"), ["src/a.css: ./gone.css"]);
}

#[test]
fn a_missing_alias_target_falls_through_to_bundler_lookup_directories() {
    let memory = MemoryFs::new()
        .with_file(
            format!("{ROOT}/package.json"),
            r#"{"name":"app","private":true,"main":"src/index.js","devDependencies":{"@rspack/core":"1.0.0","typescript":"5.0.0"},"scripts":{"build":"rspack build"}}"#,
        )
        .with_file(
            format!("{ROOT}/tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"src","paths":{"kit":["kit"]}}}"#,
        )
        .with_file(
            format!("{ROOT}/rspack.config.js"),
            "const path = require('path');\nmodule.exports = { entry: './src/index.js', resolve: { modules: [path.resolve(__dirname, '../shared'), 'node_modules'] } };\n",
        )
        // The lookup directory sits outside the project, next to it.
        .with_file("/shared/kit/index.js", "export const kit = 1;\n")
        .with_file("/shared/kit/css/index.scss", ".a {}\n")
        .with_file(
            format!("{ROOT}/src/index.js"),
            "import 'kit/css/index.scss';\nimport { kit } from 'kit';\nconsole.log(kit);\n",
        );
    let found: Vec<String> = lint_memory(memory, "")
        .iter()
        .filter(|d| matches!(d.rule.as_str(), "unresolved-imports" | "unlisted-dependencies"))
        .map(|d| d.message.clone())
        .collect();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_mastra_app_has_no_unused_files_or_dependencies() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"svc","private":true,"main":"index.js","scripts":{"build":"mastra build"},"dependencies":{"mastra":"1.0.0","@mastra/core":"1.0.0","@mastra/memory":"1.0.0"}}"#,
            ),
            (
                "src/mastra/index.ts",
                "import { Mastra } from '@mastra/core';\nimport { agent } from './agents/agent';\nimport { flow } from './workflows/flow';\nexport const mastra = new Mastra({ agent, flow });\n",
            ),
            (
                "src/mastra/agents/agent.ts",
                "import { Memory } from '@mastra/memory';\nimport { tool } from '../tools/tool';\nexport const agent = { Memory, tool };\n",
            ),
            ("src/mastra/tools/tool.ts", "export const tool = 1;\n"),
            ("src/mastra/workflows/flow.ts", "export const flow = 1;\n"),
        ],
        "",
    );
    assert!(found(&diagnostics, "unused-files").is_empty(), "{diagnostics:?}");
    assert!(found(&diagnostics, "unused-dependencies").is_empty(), "{diagnostics:?}");
}

#[test]
fn node_modules_specifiers_use_the_package_and_are_never_unresolved() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"mylib","main":"src/index.js","dependencies":{"express":"4.0.0","lodash":"4.0.0","@scope/pkg":"1.0.0"}}"#,
            ),
            (
                "src/index.js",
                "require('mylib/node_modules/express');\nrequire('../node_modules/lodash/fp');\nrequire('/abs/node_modules/@scope/pkg/x');\n",
            ),
        ],
        "",
    );
    assert!(found(&diagnostics, "unresolved-imports").is_empty(), "{diagnostics:?}");
    assert!(found(&diagnostics, "unused-dependencies").is_empty(), "{diagnostics:?}");
}

#[test]
fn entries_named_by_a_root_plugin_config_match_files_of_member_packages() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"root","private":true,"workspaces":["apps/*"],"devDependencies":{"@playwright/test":"1"}}"#,
            ),
            (
                "playwright.config.ts",
                "export default { projects: [{ testDir: './apps/web/playwright', testMatch: /.*\\.e2e\\.ts/ }] };\n",
            ),
            ("apps/web/package.json", r#"{"name":"web","private":true}"#),
            ("apps/web/src/index.ts", "export {};\n"),
            ("apps/web/playwright/signup.e2e.ts", "export const signup = 1;\n"),
            ("apps/web/orphan.ts", "export const orphan = 1;\n"),
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["apps/web/orphan.ts: apps/web/orphan.ts"]
    );
}

#[test]
fn a_package_may_import_what_the_package_that_contains_it_lists() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"root","private":true,"workspaces":["packages/*","packages/store/*"]}"#,
            ),
            (
                "packages/store/package.json",
                r#"{"name":"store","version":"1.0.0","main":"index.ts","dependencies":{"zod":"3"}}"#,
            ),
            ("packages/store/index.ts", "import 'zod';\n"),
            (
                "packages/store/app/package.json",
                r#"{"name":"app","version":"1.0.0","main":"index.ts"}"#,
            ),
            ("packages/store/app/index.ts", "import 'zod';\nimport 'missing';\n"),
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["packages/store/app/index.ts: missing"]
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), none());
}

fn nested_root_files(extra: &[(&'static str, &'static str)]) -> MemoryFs {
    let files = [
        ("/ws/.git/HEAD", "ref: refs/heads/main\n"),
        (
            "/ws/package.json",
            r#"{"name":"top","private":true,"devDependencies":{"lodash":"4","jest":"29"}}"#,
        ),
        (
            "/ws/fixtures/e2e/package.json",
            r#"{"name":"e2e","private":true,"main":"index.js"}"#,
        ),
        ("/ws/fixtures/e2e/index.js", "require('lodash');\n"),
    ];
    files
        .iter()
        .chain(extra)
        .fold(MemoryFs::new(), |memory, (path, text)| memory.with_file(path, text))
}

#[test]
fn a_nested_root_may_import_what_the_manifests_above_it_list() {
    let diagnostics = lint_memory_at(nested_root_files(&[]), "", "/ws/fixtures/e2e");
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn a_nested_root_enables_the_plugins_of_tools_the_manifests_above_it_list() {
    let memory = nested_root_files(&[
        (
            "/ws/fixtures/e2e/jest.config.js",
            "module.exports = { snapshotSerializers: ['./serializers/string.js'] };\n",
        ),
        ("/ws/fixtures/e2e/serializers/string.js", "module.exports = {};\n"),
    ]);
    let diagnostics = lint_memory_at(memory, "", "/ws/fixtures/e2e");
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
}

#[test]
fn extension_less_bin_stubs_and_flavoured_output_directories_lead_to_source() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"cli","version":"1.0.0","main":"lib-esm/index.js","bin":{"cli":"bin/cli"},"dependencies":{"commander":"1","left":"1"}}"#,
            ),
            ("bin/cli", "#!/usr/bin/env node\nrequire('../lib-commonjs/start');\n"),
            ("src/start.ts", "import 'commander';\nimport './helper';\n"),
            ("src/helper.ts", "export {};\n"),
            ("src/index.ts", "import 'left';\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn a_package_whose_published_entries_have_no_source_is_not_judged() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"lib","version":"1.0.0","main":"lib-commonjs/missing.js","dependencies":{"left":"1"}}"#,
            ),
            ("src/other.ts", "import 'left';\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn single_file_components_are_project_files() {
    let diagnostics = lint_files(
        &[
            ("package.json", r#"{"name":"app","private":true,"main":"src/index.ts"}"#),
            (
                "src/index.ts",
                "import Used from './Used.vue';\nexport const used = Used;\n",
            ),
            ("src/Used.vue", "<script>\nexport default {};\n</script>\n"),
            ("src/Orphan.vue", "<script>\nexport default {};\n</script>\n"),
            ("src/Orphan.svelte", "<script>\nexport let a = 1;\n</script>\n"),
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/Orphan.svelte: src/Orphan.svelte", "src/Orphan.vue: src/Orphan.vue"]
    );
}

#[test]
fn binaries_named_like_a_listed_package_are_not_reported_without_node_modules() {
    let diagnostics = lint_files(
        &[(
            "package.json",
            r#"{"name":"app","private":true,"scripts":{"a":"docusaurus-start","b":"nitro dev","c":"stencil build","d":"oxfmt ."},"devDependencies":{"docusaurus":"3","nitropack":"2","@stencil/core":"4"}}"#,
        )],
        "",
    );
    assert_eq!(found(&diagnostics, "unlisted-binaries"), ["package.json: oxfmt"]);
}

#[test]
fn a_namespace_import_that_is_re_exported_through_a_local_binding_keeps_its_members_used() {
    let diagnostics = lint_files(
        &[
            ("package.json", r#"{"name":"app","private":true,"main":"router.ts"}"#),
            (
                "router.ts",
                "import * as record from './record/Record.resource';\nexport function run(operation: 'create') {\n  return record[operation].execute();\n}\nexport const all = record.description;\n",
            ),
            (
                "record/Record.resource.ts",
                "import * as create from './create.operation';\nexport { create };\nexport const description = [...create.description];\n",
            ),
            (
                "record/create.operation.ts",
                "export const description = [1];\nexport function execute() {\n  return description;\n}\n",
            ),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-exports"), none(), "{diagnostics:#?}");
}

#[test]
fn out_and_target_are_skipped_only_at_the_package_root() {
    let diagnostics = lint_files(
        &[
            ("package.json", r#"{"name":"app","private":true,"main":"src/index.ts"}"#),
            ("src/index.ts", "export {};\n"),
            ("src/out/orphan.ts", "export const a = 1;\n"),
            ("out/generated.ts", "export const b = 1;\n"),
            ("target/generated.ts", "export const c = 1;\n"),
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/out/orphan.ts: src/out/orphan.ts"]
    );
}

#[test]
fn jsx_files_do_not_use_react_when_the_tsconfig_selects_another_runtime() {
    let diagnostics = lint_files(
        &[
            (
                "package.json",
                r#"{"name":"app","private":true,"main":"src/index.tsx","dependencies":{"react":"18","preact":"10"},"devDependencies":{"typescript":"5"}}"#,
            ),
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"jsx":"react-jsx","jsxImportSource":"preact"}}"#,
            ),
            ("src/index.tsx", "export const App = () => <div />;\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: react"]);
}

#[test]
fn binaries_run_in_a_named_workspace_use_the_provider_listed_above_it() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["sub"],"devDependencies":{"rimraf":"5.0.0"},"scripts":{"clean":"pnpm --filter sub exec rimraf dist"}}"#,
        ),
        ("sub/package.json", r#"{"name":"sub","private":true,"main":"index.ts"}"#),
        ("sub/index.ts", "export {};\n"),
    ];
    assert_eq!(found(&lint_files(&files, ""), "unused-dev-dependencies"), none());
    let other = [
        files[0],
        ("sub/package.json", r#"{"name":"other","private":true,"main":"index.ts"}"#),
        files[2],
    ];
    assert_eq!(
        found(&lint_files(&other, ""), "unused-dev-dependencies"),
        ["package.json: rimraf"]
    );
}

#[test]
fn script_wrappers_run_the_command_they_wrap() {
    let files = [(
        "package.json",
        r#"{"name":"app","private":true,"main":"index.ts","devDependencies":{"prettier":"3.0.0","stylelint":"16.0.0"},"scripts":{"fmt":"git ls-files | xargs prettier --write","css":"timeout 30 watch -n1 stylelint src"}}"#,
    ), ("index.ts", "export {};\n")];
    assert_eq!(found(&lint_files(&files, ""), "unused-dev-dependencies"), none());
}
