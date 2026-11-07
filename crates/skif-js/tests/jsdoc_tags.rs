//! `JSDoc` tags as knip reads them: `@public`, `@beta`, `@alpha`, `@alias` and `@internal`, the `ignore-tags` and
//! `include-tags` options of `unused-exports`, on exports and on enum and namespace members.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = PluginSource::new(config.js.plugins.clone())
        .with_workspaces(&config.js.workspaces)
        .with_overrides(&config.overrides);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `rule path: message` of the diagnostics of the export, member and hint rules, sorted.
fn report(diagnostics: &[Diagnostic]) -> String {
    let mut lines: Vec<String> = diagnostics
        .iter()
        .filter(|d| {
            [
                "unused-exports",
                "unused-types",
                "duplicate-exports",
                "unused-enum-members",
                "unused-namespace-members",
                "stale-js-config",
            ]
            .contains(&d.rule.as_str())
        })
        .map(|d| {
            format!(
                "{} {}: {}",
                d.rule,
                d.location.path.display().to_string().replace('\\', "/"),
                d.message
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

/// A package whose entry imports `lib.ts` as a namespace, so that every export of it is used only when named.
fn package(lib: &'static str, index: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", r#"{"name":"app","private":true,"main":"src/index.ts"}"#),
        ("src/index.ts", index),
        ("src/lib.ts", lib),
    ]
}

#[test]
fn an_alias_tag_exempts_an_export_from_unused_and_duplicate_exports() {
    let lib = "export const used = 1;\n/** @alias */\nexport const aliased = used;\nexport const duplicate = used;\n/** @alias */\nexport const unused = 2;\nexport const plain = 3;\n";
    let diagnostics = lint(
        &package(lib, "import { used } from \"./lib\";\nconsole.log(used);\n"),
        "",
    );
    insta::assert_snapshot!(report(&diagnostics));
}

const MEMBERS: &str = "export enum Color {\n  Red,\n  /** @internal */\n  Green,\n  /** @beta */\n  Blue,\n  /** @alpha */\n  Cyan,\n  /** @lintignore */\n  Pink,\n  Plain,\n}\nexport namespace Shapes {\n  export const circle = 1;\n  /** @internal */\n  export const square = 2;\n  /** @lintignore */\n  export const star = 3;\n  export const line = 4;\n}\n/** @lintignore */\nexport enum Ignored {\n  A,\n  B,\n}\n";

const MEMBERS_INDEX: &str =
    "import { Color, Shapes, Ignored } from \"./lib\";\nconsole.log(Color.Red, Shapes.circle, Ignored.A);\n";

#[test]
fn member_tags_exempt_enum_and_namespace_members() {
    let files = package(MEMBERS, MEMBERS_INDEX);
    insta::assert_snapshot!("member_tags_default", report(&lint(&files, "")));
    insta::assert_snapshot!(
        "member_tags_production_and_ignore_tags",
        report(&lint(
            &files,
            "[js]\nproduction = true\n[rules]\nunused-exports = { ignore-tags = [\"lintignore\"] }\n"
        ))
    );
}
