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

/// The `explain-usage` report of `file` in a workspace of `files` with the `skif.toml` text `config`, as JSON.
fn report(files: &[(&str, &str)], config: &str, file: &str) -> Value {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
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

/// A package whose vite config spreads in an unknown base, so its entry files are not all known.
const INCOMPLETE: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"app","version":"1.0.0","private":true,"main":"index.ts","devDependencies":{"vite":"6.0.0"}}"#,
    ),
    (
        "vite.config.ts",
        "import base from './vite.base';\nexport default { ...base };\n",
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
