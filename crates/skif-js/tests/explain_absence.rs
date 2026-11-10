//! Why `explain-usage` says a file outside the module graph is not analysed: one case per absence reason.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use skif_config::Config;
use skif_core::{FileSystem, MemoryFs};
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

/// A discovered declaration file is an ambient entry, so it is in the graph and `declaration-file` is not reached
/// for it; a declaration file Git ignores is not discovered.
#[test]
fn a_declaration_file() {
    let files = [
        PACKAGE,
        ("index.ts", "export const main = 1;\n"),
        ("lib/types.d.ts", "export declare const x: number;\n"),
        ("lib/generated.d.ts", "export declare const y: number;\n"),
        (".gitignore", "lib/generated.d.ts\n"),
    ];
    let types = report(&files, "", "lib/types.d.ts");
    assert_eq!(
        (&types["in-graph"], &types["entry"]),
        (&json!(true), &json!("entry-glob")),
        "{types}"
    );
    assert_eq!(absence(&files, "", "lib/generated.d.ts"), "not-discovered");
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

/// The exports of a file outside the narrowed scope are judged: every file of the scope is an entry, so whatever an
/// unknown entry imports is counted (ADR 0081). The exports of a file in the scope stay unjudged.
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
    let explain = |file: &str, export: &str| -> Value {
        let memory = memory(&files);
        let config = Config::default();
        let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(memory);
        let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(PluginSource::new(config.js.plugins.clone()))];
        let report =
            JsAnalysis::build(&workspace, &config, &fs, &sources, true).explain_usage(Path::new(file), Some(export));
        serde_json::to_value(report).unwrap()["export"].clone()
    };
    // A side-effect import uses no export.
    let helper = explain("src/helper.ts", "helper");
    assert_eq!(helper["verdict"], "unused", "{helper}");
    assert_eq!(helper.get("entries-incomplete"), None, "{helper}");
    let fixture = explain("test/db-setup.ts", "fixture");
    assert_eq!(fixture["entries-incomplete"], true, "{fixture}");
    assert_ne!(fixture["verdict"], "unused", "{fixture}");
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
/// package's entries known (ADR 0069).
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
