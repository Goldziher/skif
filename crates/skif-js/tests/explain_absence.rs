//! Why `explain-usage` says a file outside the module graph is not analysed: one case per absence reason.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use skif_config::Config;
use skif_core::{FileSystem, IncompleteScope, MemoryFs};
use skif_js::{InputSource, JsAnalysis, PluginSource};
use skif_workspace::Workspace;

// The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
const ROOT: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };

const PACKAGE: (&str, &str) = (
    "package.json",
    r#"{"name":"app","version":"1.0.0","private":true,"main":"index.ts"}"#,
);

fn memory(files: &[(&str, &str)]) -> MemoryFs {
    files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    })
}

/// The `explain-usage` report of `file` in a workspace of `files` with the `skif.toml` text `config`, as JSON.
fn report(files: &[(&str, &str)], config: &str, file: &str) -> Value {
    let memory = memory(files);
    let config = Config::from_toml_str(config).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(PluginSource::new(config.js.plugins.clone()))];
    let report = JsAnalysis::build(&workspace, &config, &fs, &sources, true).explain_usage(Path::new(file), None);
    serde_json::to_value(report).unwrap()
}

/// The `explain-usage` report of export `export` of `file`, as JSON.
fn export_report(files: &[(&str, &str)], config: &str, file: &str, export: &str) -> Value {
    let memory = memory(files);
    let config = Config::from_toml_str(config).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(PluginSource::new(config.js.plugins.clone()))];
    let report =
        JsAnalysis::build(&workspace, &config, &fs, &sources, true).explain_usage(Path::new(file), Some(export));
    serde_json::to_value(report).unwrap()["export"].clone()
}

fn absence(files: &[(&str, &str)], config: &str, file: &str) -> Value {
    let report = report(files, config, file);
    assert_eq!(report["in-graph"], false, "{report}");
    report["absence"].clone()
}

/// A package whose vite config spreads in an unknown base and computes its library entry itself, so its entry files
/// are not all known and may be any file.
const INCOMPLETE: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"app","version":"1.0.0","private":true,"main":"index.ts","devDependencies":{"vite":"6.0.0"}}"#,
    ),
    (
        "vite.config.ts",
        "import base from './vite.base';\nexport default { ...base, build: { lib: { entry: pick() } } };\n",
    ),
    ("index.ts", "export const main = 1;\n"),
    ("orphan.ts", "export const lonely = 1;\n"),
    ("README.md", "# app\n"),
    ("generated.ts", "export const generated = 1;\n"),
    (".gitignore", "generated.ts\n"),
];

#[test]
fn an_unused_file() {
    let files = [
        PACKAGE,
        ("index.ts", "export const main = 1;\n"),
        ("orphan.ts", "export const lonely = 1;\n"),
    ];
    assert_eq!(absence(&files, "", "orphan.ts"), "unused-file");
}

#[test]
fn a_file_in_an_excluded_package() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","version":"1.0.0","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "packages/a/package.json",
            r#"{"name":"a","version":"1.0.0","main":"index.ts"}"#,
        ),
        ("packages/a/index.ts", "export const a = 1;\n"),
        ("packages/a/orphan.ts", "export const lonely = 1;\n"),
    ];
    let config = "[js]\nignore-workspaces = [\"packages/a\"]\n";
    assert_eq!(absence(&files, config, "packages/a/orphan.ts"), "package-excluded");
}

/// A discovered declaration file (`.d.ts`, `.d.mts`, `.d.cts`) is an ambient entry, so it is in the graph and
/// `declaration-file` is not reached for it; a declaration file Git ignores is not discovered.
#[test]
fn a_declaration_file() {
    for extension in ["d.ts", "d.mts", "d.cts"] {
        let types_file = format!("lib/types.{extension}");
        let generated_file = format!("lib/generated.{extension}");
        let gitignore = format!("{generated_file}\n");
        let files = [
            PACKAGE,
            ("index.ts", "export const main = 1;\n"),
            (types_file.as_str(), "export declare const x: number;\n"),
            (generated_file.as_str(), "export declare const y: number;\n"),
            (".gitignore", gitignore.as_str()),
        ];
        let types = report(&files, "", &types_file);
        assert_eq!(
            (&types["in-graph"], &types["entry"]),
            (&json!(true), &json!("entry-glob")),
            "{types}"
        );
        assert_eq!(absence(&files, "", &generated_file), "not-discovered", "{extension}");
    }
}

/// A root Playwright config whose `testDir` lies in a member package makes that member's tests entries; the reason
/// names the enclosing package's glob rather than none.
#[test]
fn a_test_an_enclosing_package_names_is_an_entry_of_that_package() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["apps/*"],"devDependencies":{"@playwright/test":"1"}}"#,
        ),
        (
            "playwright.config.ts",
            "export default { testDir: './apps/web/e2e', testMatch: '**/*.e2e.ts' };\n",
        ),
        (
            "apps/web/package.json",
            r#"{"name":"web","private":true,"main":"index.ts"}"#,
        ),
        ("apps/web/index.ts", "export {};\n"),
        ("apps/web/e2e/signup.e2e.ts", "export const signup = 1;\n"),
    ];
    let test = report(&files, "", "apps/web/e2e/signup.e2e.ts");
    assert_eq!(
        (&test["in-graph"], &test["entry"]),
        (&json!(true), &json!("enclosing-entry-glob")),
        "{test}"
    );
}

#[test]
fn a_file_outside_the_project_globs() {
    let files = [
        PACKAGE,
        ("index.ts", "export const main = 1;\n"),
        ("scripts/tool.ts", "export const tool = 1;\n"),
    ];
    let config = "[js]\nproject = [\"index.ts\", \"src/**\"]\n";
    assert_eq!(absence(&files, config, "scripts/tool.ts"), "outside-project");
}

#[test]
fn an_ignored_file() {
    let files = [
        PACKAGE,
        ("index.ts", "export const main = 1;\n"),
        ("orphan.ts", "export const lonely = 1;\n"),
    ];
    assert_eq!(
        absence(&files, "[js]\nignore = [\"orphan.ts\"]\n", "orphan.ts"),
        "ignored"
    );
}

#[test]
fn files_the_analysis_never_discovers() {
    let files = [
        PACKAGE,
        ("index.ts", "export const main = 1;\n"),
        ("README.md", "# app\n"),
        ("generated.ts", "export const generated = 1;\n"),
        (".gitignore", "generated.ts\n"),
    ];
    for file in ["README.md", "generated.ts", "missing.ts"] {
        assert_eq!(absence(&files, "", file), "not-discovered", "{file}");
    }
    // A path no package owns, outside the workspace.
    let outside = if cfg!(windows) {
        "C:/elsewhere/x.ts"
    } else {
        "/elsewhere/x.ts"
    };
    assert_eq!(absence(&files, "", outside), "not-discovered");
    assert_eq!(absence(&files, "", "../elsewhere/x.ts"), "not-discovered");
}

#[test]
fn a_source_file_in_a_package_with_incomplete_entries_names_the_configs() {
    let report = report(INCOMPLETE, "", "orphan.ts");
    assert_eq!(report["absence"], "entries-incomplete", "{report}");
    assert_eq!(
        report["incomplete"],
        json!([
            { "cause": "unknown-spread", "plugin": "vite", "file": "vite.config.ts" },
            { "cause": "computed-entries", "plugin": "vite", "file": "vite.config.ts" },
        ])
    );
}

/// A package whose vite config only spreads in an unknown base: the base can at most name the conventional `index`
/// or `main` module, so only that is treated as an entry and the rest is judged.
const NARROWED: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"app","version":"1.0.0","private":true,"devDependencies":{"vite":"6.0.0"}}"#,
    ),
    (
        "vite.config.ts",
        "import base from './vite.base';\nexport default { ...base };\n",
    ),
    ("src/main.ts", "import './helper';\n"),
    ("src/helper.ts", "export const helper = 1;\n"),
    ("src/orphan.ts", "export const lonely = 1;\n"),
];

#[test]
fn a_file_outside_the_narrowed_scope_is_judged_and_names_the_scope() {
    let report = report(NARROWED, "", "src/orphan.ts");
    assert_eq!(report["absence"], "unused-file", "{report}");
    assert!(
        report["incomplete-scope"].as_array().is_some_and(|globs| globs
            .iter()
            .any(|glob| glob == "{,src/}{index,main}.{js,jsx,ts,tsx,mjs,cjs,mts,cts,vue,svelte,astro,mdx}")),
        "{report}"
    );
    assert_eq!(report["incomplete"][0]["cause"], "unknown-spread", "{report}");
    let text = JsAnalysis::build(
        &Workspace::discover_with_fs(Path::new(ROOT), &Config::default(), &memory(NARROWED)).unwrap(),
        &Config::default(),
        &(Arc::new(memory(NARROWED)) as Arc<dyn FileSystem>),
        &[Arc::new(PluginSource::new(Config::default().js.plugins.clone())) as Arc<dyn InputSource>],
        true,
    )
    .explain_usage(Path::new("src/orphan.ts"), None)
    .render();
    assert!(
        text.contains("(narrowed scope); this file does not, so it is judged"),
        "{text}"
    );
}

#[test]
fn incomplete_packages_carry_the_scope_left_unjudged() {
    let analyse = |files: &[(&str, &str)]| {
        let fs = memory(files);
        let workspace = Workspace::discover_with_fs(Path::new(ROOT), &Config::default(), &fs).unwrap();
        JsAnalysis::build(
            &workspace,
            &Config::default(),
            &(Arc::new(fs) as Arc<dyn FileSystem>),
            &[Arc::new(PluginSource::new(Config::default().js.plugins.clone())) as Arc<dyn InputSource>],
            true,
        )
        .incomplete_packages()
    };
    let narrowed = analyse(NARROWED);
    assert_eq!(narrowed.len(), 1);
    let IncompleteScope::Within(globs) = &narrowed[0].scope else {
        panic!("a narrowed scope: {:?}", narrowed[0].scope);
    };
    assert!(
        globs
            .iter()
            .any(|glob| glob == "{,src/}{index,main}.{js,jsx,ts,tsx,mjs,cjs,mts,cts,vue,svelte,astro,mdx}"),
        "{globs:?}"
    );
    let whole = analyse(INCOMPLETE);
    assert_eq!(whole.len(), 1);
    assert_eq!(whole[0].scope, IncompleteScope::Package);
}

#[test]
fn a_file_in_the_narrowed_scope_is_an_entry() {
    // A test config spreading an unknown base may name a setup module; it is treated as an entry.
    let files = [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"devDependencies":{"vitest":"3.0.0"}}"#,
        ),
        (
            "vitest.config.ts",
            "import base from './vitest.base';\nexport default { ...base };\n",
        ),
        ("src/index.ts", "export const main = 1;\n"),
        ("test/db-setup.ts", "import '../src/helper';\n"),
        ("src/helper.ts", "export const helper = 1;\n"),
    ];
    let setup = report(&files, "", "test/db-setup.ts");
    assert_eq!(
        (&setup["in-graph"], &setup["entry"]),
        (&json!(true), &json!("incomplete-scope")),
        "{setup}"
    );
    assert_eq!(setup["incomplete"][0]["plugin"], "vitest", "{setup}");
    // What the scope's files import is reached through them.
    let helper = report(&files, "", "src/helper.ts");
    assert_eq!(
        helper["chain-from-entry"],
        json!(["test/db-setup.ts", "src/helper.ts"]),
        "{helper}"
    );
}

/// The narrowed scope of an incomplete root reaches the files of the members below it (ADR 0016); the reason names
/// that enclosing scope and its package rather than none.
#[test]
fn a_member_file_in_the_narrowed_scope_of_the_root_is_an_entry_of_that_scope() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"vitest":"3.0.0"}}"#,
        ),
        (
            "vitest.config.ts",
            "import base from './vitest.base';\nexport default { ...base };\n",
        ),
        (
            "packages/core/package.json",
            r#"{"name":"core","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        ("packages/core/src/index.ts", "export const main = 1;\n"),
        ("packages/core/src/test-utils/mock.ts", "export const mock = 1;\n"),
    ];
    let mock = report(&files, "", "packages/core/src/test-utils/mock.ts");
    assert_eq!(
        (&mock["in-graph"], &mock["entry"], &mock["scope-package"]),
        (&json!(true), &json!("enclosing-incomplete-scope"), &json!(".")),
        "{mock}"
    );
    assert_eq!(mock["incomplete"][0]["plugin"], "vitest", "{mock}");
    assert!(
        mock["incomplete-scope"].as_array().is_some_and(|globs| globs
            .iter()
            .any(|glob| glob.as_str().is_some_and(|glob| glob.contains("test-utils")))),
        "{mock}"
    );
    // A file of the member's own scope-less package keeps its own reason, without a scope package.
    let index = report(&files, "", "packages/core/src/index.ts");
    assert_eq!(
        (&index["entry"], index.get("scope-package")),
        (&json!("entry-glob"), None),
        "{index}"
    );
}

/// The exports of a file outside the narrowed scope are judged: every file of the scope is an entry, so whatever an
/// unknown entry imports is counted (ADR 0016). The exports of a file in the scope stay unjudged.
#[test]
fn an_export_outside_the_narrowed_scope_is_judged() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"devDependencies":{"vitest":"3.0.0"}}"#,
        ),
        (
            "vitest.config.ts",
            "import base from './vitest.base';\nexport default { ...base };\n",
        ),
        ("src/index.ts", "export const main = 1;\n"),
        (
            "test/db-setup.ts",
            "import '../src/helper';\nexport const fixture = 1;\n",
        ),
        ("src/helper.ts", "export const helper = 1;\n"),
    ];
    let explain = |file: &str, export: &str| export_report(&files, "", file, export);
    // A side-effect import uses no export.
    let helper = explain("src/helper.ts", "helper");
    assert_eq!(helper["verdict"], "unused", "{helper}");
    assert_eq!(helper.get("entries-incomplete"), None, "{helper}");
    let fixture = explain("test/db-setup.ts", "fixture");
    assert_eq!(fixture["entries-incomplete"], true, "{fixture}");
    // The file of the scope is an entry, whose exports are public.
    assert_eq!(fixture["verdict"], "public", "{fixture}");
}

/// A computed `import()` may load any file of the package, so a narrowed scope is not used.
#[test]
fn a_computed_import_keeps_the_package_unjudged() {
    let mut files = NARROWED.to_vec();
    files[2] = (
        "src/main.ts",
        "import './helper';\nexport const load = (name: string) => import(name);\n",
    );
    let report = report(&files, "", "src/orphan.ts");
    assert_eq!(report["absence"], "entries-incomplete", "{report}");
    assert_eq!(report["widened-by"], "src/main.ts:2", "{report}");
    // Nor are the exports outside the scope judged.
    let helper = export_report(&files, "", "src/helper.ts", "helper");
    assert_eq!(helper["entries-incomplete"], true, "{helper}");
    assert_eq!(helper["verdict"], "not-judged", "{helper}");
}

/// Incomplete entries only explain a source file the analysis would otherwise judge.
#[test]
fn incomplete_entries_do_not_hide_what_discovery_and_ignores_leave_out() {
    assert_eq!(absence(INCOMPLETE, "", "README.md"), "not-discovered");
    assert_eq!(absence(INCOMPLETE, "", "generated.ts"), "not-discovered");
    assert_eq!(absence(INCOMPLETE, "", "missing.ts"), "not-discovered");
    assert_eq!(
        absence(INCOMPLETE, "[js]\nignore = [\"orphan.ts\"]\n", "orphan.ts"),
        "ignored"
    );
    assert_eq!(
        absence(INCOMPLETE, "[js]\nproject = [\"index.ts\"]\n", "orphan.ts"),
        "outside-project"
    );
}

#[test]
fn published_build_output_without_a_source() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"main":"./dist/index.js"}"#,
        ),
        ("lib/index.ts", "export const main = 1;\n"),
        ("orphan.ts", "export const lonely = 1;\n"),
    ];
    let report = report(&files, "", "orphan.ts");
    assert_eq!(report["absence"], "entries-incomplete", "{report}");
    assert_eq!(report["incomplete"][0]["cause"], "unmapped-published", "{report}");
}

/// A published path into nothing (no file, no source behind it, not build output) contributes no entry and leaves the
/// package's entries known (ADR 0016).
#[test]
fn a_published_path_into_nothing_leaves_the_package_judged() {
    let files = [
        (
            "package.json",
            r#"{"name":"r2b","private":true,"bin":{"r2b":"bin/r2b.js"},"scripts":{"start":"node app/main.js"}}"#,
        ),
        ("app/main.js", "console.log('r2b');\n"),
        ("app/orphan.js", "export const lonely = 1;\n"),
    ];
    assert_eq!(absence(&files, "", "app/orphan.js"), "unused-file");
}

/// A webpack config bundling `entry` from a package with only `src/other.ts` and `orphan.ts`.
fn bundling(entry: &str) -> [(&'static str, String); 4] {
    [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"devDependencies":{"webpack":"5.0.0"}}"#.to_owned(),
        ),
        (
            "webpack.config.js",
            format!(
                "const path = require('path');\n\
                 module.exports = () => ({{ entry: {{ bundle: path.join(__dirname, {entry}) }} }});\n"
            ),
        ),
        ("src/other.ts", "export const other = 1;\n".to_owned()),
        ("orphan.ts", "export const lonely = 1;\n".to_owned()),
    ]
}

/// A plugin entry in build output that maps back to no source file hides what the bundle reaches, so the package's
/// entries are incomplete and name the entry (ADR 0035).
#[test]
fn a_plugin_entry_in_build_output_without_a_source() {
    let files = bundling("'lib-esm', 'entry.js'");
    let files: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
    let report = report(&files, "", "orphan.ts");
    assert_eq!(report["absence"], "entries-incomplete", "{report}");
    assert_eq!(
        report["incomplete"],
        json!([{ "cause": "unmapped-entry", "file": "lib-esm/entry.js" }]),
        "{report}"
    );
}

/// A plugin entry into nothing (no file, no source behind it, not build output) contributes no entry and leaves the
/// package's entries known, as a published path into nothing does (ADR 0016).
#[test]
fn a_plugin_entry_into_nothing_leaves_the_package_judged() {
    let files = bundling("'app', 'entry.js'");
    let files: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
    assert_eq!(absence(&files, "", "orphan.ts"), "unused-file");
}

/// A plugin entry in build output that is no code file (a compiled binary a workflow runs) needs no source behind it:
/// it contributes no entry and leaves the package's entries known.
#[test]
fn a_plugin_entry_to_a_built_binary_leaves_the_package_judged() {
    let files = bundling("'lib', 'release', 'tool'");
    let files: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
    assert_eq!(absence(&files, "", "orphan.ts"), "unused-file");
}

/// An `export *` forwards every export without declaring it: the walk follows the forwarded name through the
/// re-exporting module's users instead of calling it no export of that module, and the importer list says the module
/// is re-exported as a whole.
#[test]
fn an_export_forwarded_by_export_star_is_followed_through_its_users() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"main":"src/main.ts"}"#,
        ),
        ("src/main.ts", "import { foo } from './lib';\nconsole.log(foo);\n"),
        ("src/lib.ts", "export * from './utils';\n"),
        ("src/utils.ts", "export const foo = 1;\n"),
    ];
    let usage = report(&files, "", "src/utils.ts");
    assert_eq!(
        usage["imported-by"][0]["names"],
        json!(["every export, re-exported"]),
        "{usage}"
    );
    let export = export_report(&files, "", "src/utils.ts", "foo");
    assert_eq!(export["verdict"], "used", "{export}");
    let forwarded = &export["re-exports"][0];
    assert_eq!(forwarded["file"], "src/lib.ts", "{export}");
    assert_eq!(forwarded["export"]["verdict"], "used", "{export}");
    assert_eq!(forwarded["export"]["users"][0]["file"], "src/main.ts", "{export}");
}

/// The import skif adds where a template uses a registered component has no location: it is listed with line 0 and
/// rendered as a template use, not as line 1 of the template's file.
#[test]
fn a_registered_component_a_template_uses_is_listed_without_a_line() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","private":true,"dependencies":{"vue":"3.5.0"},"devDependencies":{"unplugin-vue-components":"28.0.0","vite":"6.0.0"}}"#,
        ),
        (
            "vite.config.ts",
            "import Components from \"unplugin-vue-components/vite\";\nimport { defineConfig } from \"vite\";\n\nexport default defineConfig({ plugins: [Components()] });\n",
        ),
        (
            "index.html",
            "<div id=\"app\"></div>\n<script type=\"module\" src=\"/src/main.ts\"></script>\n",
        ),
        (
            "components.d.ts",
            "/* eslint-disable */\n// Generated by unplugin-vue-components\nexport {}\n\ndeclare module 'vue' {\n  export interface GlobalComponents {\n    AppButton: typeof import('./src/components/AppButton.vue')['default']\n  }\n}\n",
        ),
        (
            "src/main.ts",
            "import { createApp } from \"vue\";\nimport App from \"./App.vue\";\n\ncreateApp(App).mount(\"#app\");\n",
        ),
        (
            "src/App.vue",
            "<script setup lang=\"ts\">\nconst a = 1;\n</script>\n\n<template>\n  <app-button />\n</template>\n",
        ),
        (
            "src/components/AppButton.vue",
            "<template>\n  <button />\n</template>\n",
        ),
    ];
    let usage = report(&files, "", "src/components/AppButton.vue");
    let importers = usage["imported-by"].as_array().unwrap();
    let from_app: Vec<&Value> = importers
        .iter()
        .filter(|importer| importer["file"] == "src/App.vue")
        .collect();
    assert_eq!(from_app.len(), 1, "{usage}");
    assert_eq!(from_app[0]["line"], 0, "{usage}");
}

/// A `rules_js` package whose `ts_project` builds from `a.ts` and `b.ts`, which imports `c.ts`, and whose `js_binary`
/// runs `tool.mjs`.
const BAZEL_SOURCES: &[(&str, &str)] = &[
    (
        "MODULE.bazel",
        "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
    ),
    ("package.json", r#"{"name":"app","version":"1.0.0","private":true}"#),
    (
        "BUILD.bazel",
        "ts_project(name = \"lib\", srcs = [\"a.ts\", \"b.ts\"])\n\njs_binary(name = \"tool\", entry_point = \"tool.mjs\")\n",
    ),
    ("a.ts", "export const unusedInA = 1;\n"),
    ("b.ts", "import { helper } from './c';\nhelper();\n"),
    ("c.ts", "export function helper(): void {}\n"),
    ("tool.mjs", "export const toolOption = 1;\n"),
];

/// A file a Bazel rule builds from is a build source: in the graph and the start of the chains to what it imports, but
/// no entry, so its exports are judged. The `entry_point` of a `js_binary` stays an entry with public exports.
#[test]
fn a_bazel_source_is_a_build_source_whose_exports_are_judged() {
    let a = report(BAZEL_SOURCES, "", "a.ts");
    assert_eq!(
        (&a["in-graph"], &a["entry"], &a["build-source"], &a["chain-from-entry"]),
        (&json!(true), &Value::Null, &json!(true), &json!(["a.ts"])),
        "{a}"
    );
    let c = report(BAZEL_SOURCES, "", "c.ts");
    assert_eq!(
        (
            &c["chain-from-entry"],
            &c["chain-from-build-source"],
            c.get("build-source")
        ),
        (&json!(["b.ts", "c.ts"]), &json!(true), None),
        "{c}"
    );
    let tool = report(BAZEL_SOURCES, "", "tool.mjs");
    assert_eq!(
        (
            &tool["entry"],
            tool.get("build-source"),
            tool.get("chain-from-build-source")
        ),
        (&json!("entry-glob"), None, None),
        "{tool}"
    );
    let unused = export_report(BAZEL_SOURCES, "", "a.ts", "unusedInA");
    assert_eq!(
        (&unused["verdict"], &unused["entry-public"]),
        (&json!("unused"), &json!(false)),
        "{unused}"
    );
    let public = export_report(BAZEL_SOURCES, "", "tool.mjs", "toolOption");
    assert_eq!(
        (&public["verdict"], &public["entry-public"]),
        (&json!("public"), &json!(true)),
        "{public}"
    );
}

/// The text of `explain-usage` names a build source and the chain that starts at one.
#[test]
fn the_text_report_names_build_sources() {
    let memory = memory(BAZEL_SOURCES);
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(PluginSource::new(config.js.plugins.clone()))];
    let analysis = JsAnalysis::build(&workspace, &config, &fs, &sources, true);
    let a = analysis.explain_usage(Path::new("a.ts"), None).render();
    assert!(
        a.contains("  build source: a plugin names it as a file a tool builds from"),
        "{a}"
    );
    let c = analysis.explain_usage(Path::new("c.ts"), None).render();
    assert!(c.contains("  reached from a build source through: b.ts -> c.ts"), "{c}");
}

/// A file that a `ts_project` builds from and that `package.json` publishes as its `main` is an entry, not a build
/// source, and its export is public.
#[test]
fn a_published_bazel_source_is_an_entry_not_a_build_source() {
    let files = [
        (
            "MODULE.bazel",
            "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
        ),
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"main":"app.ts"}"#,
        ),
        ("BUILD.bazel", "ts_project(name = \"lib\", srcs = [\"app.ts\"])\n"),
        ("app.ts", "export const api = 1;\n"),
    ];
    let app = report(&files, "", "app.ts");
    assert_eq!(
        (
            &app["entry"],
            app.get("build-source"),
            &app["chain-from-entry"],
            app.get("chain-from-build-source")
        ),
        (&json!("published"), None, &json!(["app.ts"]), None),
        "{app}"
    );
    let api = export_report(&files, "", "app.ts", "api");
    assert_eq!(
        (&api["verdict"], &api["entry-public"]),
        (&json!("public"), &json!(true)),
        "{api}"
    );
}

/// A use of a namespace as a whole is listed as a user of a value export and not of a type export, which it cannot
/// read: the type's verdict and its evidence agree.
#[test]
fn a_whole_namespace_use_is_listed_for_values_and_not_for_types() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import * as shapes from './shapes';\nexport const all = Object.keys(shapes);\n",
        ),
        ("shapes.ts", "export const circle = 1;\nexport type Id = string;\n"),
    ];
    let value = export_report(&files, "", "shapes.ts", "circle");
    assert_eq!(value["verdict"], "used", "{value}");
    assert_eq!(value["users"][0]["file"], "index.ts", "{value}");
    assert_eq!(value["users"][0]["how"], "the namespace, used as a whole", "{value}");
    let type_export = export_report(&files, "", "shapes.ts", "Id");
    assert_eq!(type_export["verdict"], "unused", "{type_export}");
    assert_eq!(type_export["users"], json!([]), "{type_export}");
}

/// A file another loads with `new URL("./x", import.meta.url)` runs as a program of its own, so its type exports are
/// used as a whole, as the verdict of a lint run says, and not only its values.
#[test]
fn a_new_url_target_lists_the_loader_as_a_user_of_a_type_export() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "export const asset = new URL('./generated.ts', import.meta.url);\n",
        ),
        ("generated.ts", "export type A = { a: 1 };\nexport const c = 1;\n"),
    ];
    let type_export = export_report(&files, "", "generated.ts", "A");
    assert_eq!(type_export["verdict"], "used", "{type_export}");
    assert_eq!(type_export["users"][0]["file"], "index.ts", "{type_export}");
    assert_eq!(type_export["users"][0]["how"], "the whole module", "{type_export}");
}

/// The source of the build output a script runs and a file an extensionless `bin` stub loads are entries for those
/// reasons, which the report names instead of `unknown`. The `typescript` plugin reads the build directory from
/// `tsconfig.json`, so the fixture depends on it.
#[test]
fn a_built_script_source_and_a_file_a_bin_stub_loads_name_their_entry_rule() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"bin":{"tool":"bin/tool"},"scripts":{"cov":"node dist/report.js"},"devDependencies":{"typescript":"5.6.0"}}"#,
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"outDir":"dist","rootDir":"src"}}"#,
        ),
        ("src/report.ts", "export const report = 1;\n"),
        ("bin/tool", "#!/usr/bin/env node\nrequire('./common.js');\n"),
        ("bin/common.js", "module.exports = 1;\n"),
    ];
    for (file, entry) in [("src/report.ts", "script"), ("bin/common.js", "bin-stub")] {
        let usage = report(&files, "", file);
        assert_eq!(
            (&usage["in-graph"], &usage["entry"]),
            (&json!(true), &json!(entry)),
            "{file}: {usage}"
        );
    }
}
