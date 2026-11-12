//! `[js.compilers]`: files of extensions skif has no reader for, read by declared regular expressions or with one of
//! skif's readers, join the project and their imports count as usage.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const MANIFEST: (&str, &str) = (
    "package.json",
    r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
);

const HTML: &str = r#"[js.compilers]
html = { imports = ['<script[^>]+src="([^"]+)"'] }
"#;

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = PluginSource::new(config.js.plugins.clone()).with_workspaces(&config.js.workspaces);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `path: message` of the diagnostics of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display().to_string().replace('\\', "/"),
                d.message
            )
        })
        .collect();
    found.sort();
    found
}

fn paths(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    found(diagnostics, rule)
        .into_iter()
        .map(|line| line.split(": ").next().unwrap().to_owned())
        .collect()
}

/// The issue's reproduction: `index.html` loads `src/app.ts` with a script tag, and nothing else imports it.
fn html_package() -> Vec<(&'static str, &'static str)> {
    vec![
        MANIFEST,
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        (
            "packages/a/index.html",
            "<!doctype html>\n<html>\n  <body>\n    <script type=\"module\" src=\"./src/app.ts\"></script>\n\
             <script src=\"./src/widget.ts\" defer></script>\n  </body>\n</html>\n",
        ),
        (
            "packages/a/src/app.ts",
            "import { helper } from \"./helper\";\nconsole.log(helper);\n",
        ),
        ("packages/a/src/helper.ts", "export const helper = 1;\n"),
        ("packages/a/src/widget.ts", "document.title = \"x\";\n"),
        ("packages/a/src/orphan.ts", "export const orphan = 1;\n"),
    ]
}

const HTML_ENTRY: &str = "[js.workspaces.\"packages/a\"]\nentry = [\"index.html\"]\n";

#[test]
fn script_tags_captured_by_an_imports_pattern_make_what_they_load_used() {
    let files = html_package();
    let without = lint(&files, HTML_ENTRY);
    assert_eq!(
        paths(&without, "unused-files"),
        [
            "packages/a/src/app.ts",
            "packages/a/src/helper.ts",
            "packages/a/src/orphan.ts",
            "packages/a/src/widget.ts",
        ]
    );

    let with = lint(&files, &format!("{HTML}{HTML_ENTRY}"));
    assert_eq!(paths(&with, "unused-files"), ["packages/a/src/orphan.ts"], "{with:#?}");
    assert_eq!(found(&with, "unresolved-imports"), Vec::<String>::new());
}

#[test]
fn a_compiler_file_nothing_loads_is_an_unused_project_file() {
    let mut files = html_package();
    files.push(("packages/a/public/stray.html", "<script src=\"../src/orphan.ts\"></script>\n"));
    let with = lint(&files, &format!("{HTML}{HTML_ENTRY}"));
    assert_eq!(
        paths(&with, "unused-files"),
        ["packages/a/public/stray.html", "packages/a/src/orphan.ts"],
        "{with:#?}"
    );

    // A `project` of the workspace that leaves the extension out leaves such files unjudged.
    let narrowed = lint(
        &files,
        &format!("{HTML}{HTML_ENTRY}project = [\"src/**/*.ts\"]\n"),
    );
    assert_eq!(paths(&narrowed, "unused-files"), ["packages/a/src/orphan.ts"]);
}

#[test]
fn an_extracted_import_is_unresolved_or_unlisted_where_a_written_one_would_be() {
    let files = vec![
        MANIFEST,
        ("packages/a/package.json", r#"{"name":"@x/a","dependencies":{"listed":"1"}}"#),
        (
            "packages/a/index.html",
            "<script src=\"./src/missing.ts\"></script>\n<script src=\"some-lib/dist/x.js\"></script>\n\
             <script src=\"listed\"></script>\n",
        ),
    ];
    let with = lint(&files, &format!("{HTML}{HTML_ENTRY}"));
    assert_eq!(
        found(&with, "unresolved-imports"),
        ["packages/a/index.html: cannot resolve `./src/missing.ts`"]
    );
    assert_eq!(
        found(&with, "unlisted-dependencies"),
        ["packages/a/index.html: `some-lib` is imported but not listed in `package.json`"]
    );
    assert_eq!(found(&with, "unused-dependencies"), Vec::<String>::new());

    let without = lint(&files, HTML_ENTRY);
    assert_eq!(found(&without, "unlisted-dependencies"), Vec::<String>::new());
}

#[test]
fn a_file_mapped_onto_the_mdx_reader_is_read_as_mdx() {
    let files = vec![
        MANIFEST,
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ("packages/a/src/index.ts", "import \"./intro.doc\";\n"),
        (
            "packages/a/src/intro.doc",
            "import { Card } from \"./card\";\n\n# Intro\n\nSome prose that says import \"./not-code\".\n\n<Card />\n",
        ),
        (
            "packages/a/src/card.ts",
            "export const Card = () => null;\nexport const Unused = 1;\n",
        ),
    ];
    let without = lint(&files, "");
    assert_eq!(
        paths(&without, "unused-files"),
        ["packages/a/src/card.ts"],
        "{without:#?}"
    );

    let with = lint(&files, "[js.compilers]\ndoc = { as = \"mdx\" }\n");
    assert_eq!(paths(&with, "unused-files"), Vec::<String>::new(), "{with:#?}");
    assert_eq!(found(&with, "unresolved-imports"), Vec::<String>::new());
    assert_eq!(
        found(&with, "unused-exports"),
        ["packages/a/src/card.ts: export `Unused` is never imported by another module"]
    );
}

#[test]
fn a_project_pattern_naming_a_compiler_extension_is_no_unanalysed_extension() {
    let files = html_package();
    let project = format!("{HTML_ENTRY}project = [\"**/*.{{ts,html}}\"]\n");
    let unregistered = |diagnostics: &[Diagnostic]| -> Vec<String> {
        found(diagnostics, "stale-js-config")
            .into_iter()
            .filter(|line| line.contains("`.html` files selected by"))
            .collect()
    };
    assert_eq!(unregistered(&lint(&files, &project)).len(), 1);
    assert_eq!(unregistered(&lint(&files, &format!("{HTML}{project}"))), Vec::<String>::new());
}
