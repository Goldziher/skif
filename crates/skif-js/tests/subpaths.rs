//! `unused-subpath-imports` and `unused-subpath-exports`: the `imports` and `exports` entries of a `package.json` that
//! no file imports.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, FixSafety, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";
const IMPORTS: &str = "unused-subpath-imports";
const EXPORTS: &str = "unused-subpath-exports";
const WORKSPACE: &str = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
/// Turns `unused-subpath-exports`, which is off by default, on.
const EXPORTS_ON: &str = "[rules]\nunused-subpath-exports = \"warn\"\n";

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// The findings of `rule` as `<file>: <key>`, the key being the first backticked part of the message, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            let start = d.message.find('`').map_or(0, |i| i + 1);
            let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
            format!("{}: {}", d.location.path.display(), &d.message[start..end])
        })
        .collect();
    found.sort();
    found
}

const REPRO_MANIFEST: &str = r##"{
  "name": "a",
  "type": "module",
  "imports": { "#used": "./src/used.ts", "#unused": "./src/unused.ts" },
  "exports": { ".": "./src/index.ts", "./extra": "./src/extra.ts" }
}
"##;

/// The reproduction of the issue: `#unused` is imported by no file of `a`, and `./extra` by no workspace package.
fn repro() -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", WORKSPACE),
        ("packages/a/package.json", REPRO_MANIFEST),
        (
            "packages/a/src/index.ts",
            "import { x } from '#used';\nexport const y = x;\n",
        ),
        ("packages/a/src/used.ts", "export const x = 1;\n"),
        ("packages/a/src/unused.ts", "export const z = 1;\n"),
        ("packages/a/src/extra.ts", "export const e = 1;\n"),
    ]
}

#[test]
fn the_repro_reports_the_unused_import_and_leaves_exports_to_the_opt_in_rule() {
    let diagnostics = lint(&repro(), "");
    assert_eq!(
        found(&diagnostics, IMPORTS),
        ["packages/a/package.json: #unused"],
        "{diagnostics:#?}"
    );
    assert!(found(&diagnostics, EXPORTS).is_empty(), "{diagnostics:#?}");
}

#[test]
fn the_repro_reports_every_subpath_no_workspace_imports_when_the_exports_rule_is_on() {
    // As knip with `includeSubpathExports`: nothing imports `a` by name, so `.` is unused as well as `./extra`.
    let diagnostics = lint(&repro(), EXPORTS_ON);
    assert_eq!(
        found(&diagnostics, EXPORTS),
        ["packages/a/package.json: .", "packages/a/package.json: ./extra"],
        "{diagnostics:#?}"
    );
    assert_eq!(found(&diagnostics, IMPORTS), ["packages/a/package.json: #unused"]);
}

#[test]
fn an_unused_import_points_at_its_key_and_has_an_unsafe_fix_that_removes_it() {
    let diagnostics = lint(&repro(), "");
    let finding = diagnostics.iter().find(|d| d.rule.as_str() == IMPORTS).unwrap();
    let span = finding.location.span.unwrap();
    assert_eq!(&REPRO_MANIFEST[span.start as usize..span.end as usize], "\"#unused\"");
    assert_eq!(span.start_line, 4);
    assert_eq!(finding.package.as_deref(), Some("a"));
    assert_eq!(finding.fixes.len(), 1);
    assert_eq!(finding.fixes[0].safety, FixSafety::Unsafe);
    assert_eq!(finding.fixes[0].title, "Remove `#unused` from `imports`");
}

#[test]
fn an_unused_export_points_at_its_key_and_has_no_fix() {
    let diagnostics = lint(&repro(), EXPORTS_ON);
    let finding = diagnostics
        .iter()
        .find(|d| d.rule.as_str() == EXPORTS && d.message.starts_with("`./extra`"))
        .unwrap();
    let span = finding.location.span.unwrap();
    assert_eq!(&REPRO_MANIFEST[span.start as usize..span.end as usize], "\"./extra\"");
    assert!(finding.fixes.is_empty(), "{finding:#?}");
}

#[test]
fn imports_are_matched_as_node_selects_them() {
    let manifest = r##"{"name":"a","type":"module","main":"./src/index.ts","imports":{
        "#cond": {"node": "./src/c.node.ts", "default": "./src/c.ts"},
        "#cond-unused": {"import": "./src/c.ts", "default": "./src/c.ts"},
        "#w/*": "./src/w/*.ts",
        "#v/*": "./src/v/*.ts",
        "#u/*": "./src/u/*.ts",
        "#u/special": "./src/special.ts",
        "#hidden": null,
        "#queried": "./src/q.ts"
    }}"##;
    let files = [
        ("package.json", WORKSPACE),
        ("packages/a/package.json", manifest),
        (
            "packages/a/src/index.ts",
            "import { c } from '#cond';\nimport { w } from '#w/one';\nimport { s } from '#u/special';\n\
             import q from '#queried?raw';\nexport const all = [c, w, s, q];\n",
        ),
        ("packages/a/src/c.ts", "export const c = 1;\n"),
        ("packages/a/src/c.node.ts", "export const c = 1;\n"),
        ("packages/a/src/w/one.ts", "export const w = 1;\n"),
        ("packages/a/src/special.ts", "export const s = 1;\n"),
        ("packages/a/src/q.ts", "export default 1;\n"),
    ];
    let diagnostics = lint(&files, "");
    // An exact key wins over a pattern that matches too, so `#u/*` is unused; a `null` entry hides a subpath and is
    // not reported.
    assert_eq!(
        found(&diagnostics, IMPORTS),
        [
            "packages/a/package.json: #cond-unused",
            "packages/a/package.json: #u/*",
            "packages/a/package.json: #v/*",
        ],
        "{diagnostics:#?}"
    );
}

#[test]
fn an_import_counts_only_for_the_package_of_the_importing_file() {
    // `#x` of `b` is imported, but by a file of `a`, where it is `a`'s own `#x`.
    let files = [
        ("package.json", WORKSPACE),
        (
            "packages/a/package.json",
            r##"{"name":"a","main":"./src/index.ts","imports":{"#x":"./src/x.ts"}}"##,
        ),
        ("packages/a/src/index.ts", "export { x } from '#x';\n"),
        ("packages/a/src/x.ts", "export const x = 1;\n"),
        (
            "packages/b/package.json",
            r##"{"name":"b","main":"./src/index.ts","imports":{"#x":"./src/x.ts"}}"##,
        ),
        ("packages/b/src/index.ts", "export const b = 1;\n"),
        ("packages/b/src/x.ts", "export const x = 1;\n"),
    ];
    let diagnostics = lint(&files, "");
    assert_eq!(
        found(&diagnostics, IMPORTS),
        ["packages/b/package.json: #x"],
        "{diagnostics:#?}"
    );
}

/// `a` exports subpaths that `b` imports (`./other`, a pattern), that only `a` imports by its own name (`./self`),
/// that nobody imports (`./extra`), and files that are no modules (`./style.css`, `./package.json`).
fn exporting_workspace() -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", WORKSPACE),
        (
            "packages/a/package.json",
            r#"{"name":"a","type":"module","exports":{
                ".": "./src/index.ts",
                "./extra": "./src/extra.ts",
                "./other": {"types": "./src/other.ts", "default": "./src/other.ts"},
                "./self": "./src/self.ts",
                "./feat/*": "./src/feat/*.ts",
                "./style.css": "./src/style.css",
                "./package.json": "./package.json"
            }}"#,
        ),
        (
            "packages/a/src/index.ts",
            "import { s } from 'a/self';\nexport const a = s;\n",
        ),
        ("packages/a/src/extra.ts", "export const e = 1;\n"),
        ("packages/a/src/other.ts", "export const o = 1;\n"),
        ("packages/a/src/self.ts", "export const s = 1;\n"),
        ("packages/a/src/feat/one.ts", "export const f = 1;\n"),
        ("packages/a/src/style.css", ".a {}\n"),
        (
            "packages/b/package.json",
            r#"{"name":"b","type":"module","main":"./src/index.ts","dependencies":{"a":"workspace:*"}}"#,
        ),
        (
            "packages/b/src/index.ts",
            "import { a } from 'a';\nimport { o } from 'a/other';\nimport { f } from 'a/feat/one';\n\
             export const b = [a, o, f];\n",
        ),
    ]
}

#[test]
fn an_export_is_used_when_any_workspace_package_imports_it_by_name() {
    let diagnostics = lint(&exporting_workspace(), EXPORTS_ON);
    assert_eq!(
        found(&diagnostics, EXPORTS),
        ["packages/a/package.json: ./extra"],
        "{diagnostics:#?}"
    );
}

#[test]
fn a_string_exports_field_is_the_root_subpath() {
    let files = [
        ("package.json", WORKSPACE),
        ("packages/a/package.json", r#"{"name":"a","exports":"./src/index.ts"}"#),
        ("packages/a/src/index.ts", "export const a = 1;\n"),
    ];
    let diagnostics = lint(&files, EXPORTS_ON);
    assert_eq!(
        found(&diagnostics, EXPORTS),
        ["packages/a/package.json: ."],
        "{diagnostics:#?}"
    );
    let span = diagnostics
        .iter()
        .find(|d| d.rule.as_str() == EXPORTS)
        .and_then(|d| d.location.span)
        .unwrap();
    assert_eq!(&files[1].1[span.start as usize..span.end as usize], "\"exports\"");
}

#[test]
fn production_mode_reports_neither() {
    let production = format!("{EXPORTS_ON}[js]\nproduction = true\n");
    let diagnostics = lint(&repro(), &production);
    assert!(found(&diagnostics, IMPORTS).is_empty(), "{diagnostics:#?}");
    assert!(found(&diagnostics, EXPORTS).is_empty(), "{diagnostics:#?}");
}

#[test]
fn an_ignore_entry_suppresses_the_findings_of_its_package() {
    let ignore = format!(
        "{EXPORTS_ON}[[ignore]]\nrules = [\"unused-subpath-imports\", \"unused-subpath-exports\"]\npackages = [\"a\"]\n"
    );
    let diagnostics = lint(&repro(), &ignore);
    assert!(found(&diagnostics, IMPORTS).is_empty(), "{diagnostics:#?}");
    assert!(found(&diagnostics, EXPORTS).is_empty(), "{diagnostics:#?}");
}

#[test]
fn a_package_left_out_of_the_analysis_is_not_judged() {
    let ignored = format!("{EXPORTS_ON}[js]\nignore-workspaces = [\"packages/a\"]\n");
    let diagnostics = lint(&repro(), &ignored);
    assert!(found(&diagnostics, IMPORTS).is_empty(), "{diagnostics:#?}");
    assert!(found(&diagnostics, EXPORTS).is_empty(), "{diagnostics:#?}");
}

#[test]
fn exports_a_package_left_out_of_the_analysis_may_import_are_not_judged() {
    // `b` lists `a` but is not analysed, so which subpaths of `a` it imports is unknown.
    let ignored = format!("{EXPORTS_ON}[js]\nignore-workspaces = [\"packages/b\"]\n");
    let diagnostics = lint(&exporting_workspace(), &ignored);
    assert!(found(&diagnostics, EXPORTS).is_empty(), "{diagnostics:#?}");
}
