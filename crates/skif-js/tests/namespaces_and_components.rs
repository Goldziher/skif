//! Namespace-only modules, namespace re-exports, single-file components and the virtual modules plugins declare.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

fn lint_files(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
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

const PACKAGE: (&str, &str) = ("package.json", r#"{"name":"app","private":true,"main":"index.ts"}"#);

const SHAPES: (&str, &str) = (
    "shapes.ts",
    "export function circle(r: number) { return r; }\nexport function square(s: number) { return s; }\nexport interface Shape { sides: number }\nexport type Id = string;\n",
);

#[test]
fn a_module_only_read_through_a_namespace_reports_its_unread_exports_under_the_namespace_rules() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            (
                "index.ts",
                "import * as shapes from './shapes';\nexport const area = shapes.circle(1);\n",
            ),
            SHAPES,
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-namespace-exports"),
        ["shapes.ts: square"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-namespace-types"),
        ["shapes.ts: Id", "shapes.ts: Shape"],
        "{diagnostics:#?}"
    );
    assert_eq!(found(&diagnostics, "unused-exports"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-types"), none(), "{diagnostics:#?}");
}

#[test]
fn a_type_read_through_the_namespace_is_used() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            (
                "index.ts",
                "import * as shapes from './shapes';\nexport const area: shapes.Shape = { sides: shapes.circle(1) };\n",
            ),
            SHAPES,
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-namespace-types"),
        ["shapes.ts: Id"],
        "{diagnostics:#?}"
    );
}

#[test]
fn a_module_that_is_also_imported_by_name_keeps_the_plain_rules() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            (
                "index.ts",
                "import { circle } from './shapes';\nexport const area = circle(1);\n",
            ),
            SHAPES,
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        ["shapes.ts: square"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-types"),
        ["shapes.ts: Id", "shapes.ts: Shape"]
    );
    assert_eq!(found(&diagnostics, "unused-namespace-exports"), none());
    assert_eq!(found(&diagnostics, "unused-namespace-types"), none());
}

#[test]
fn a_namespace_used_as_a_whole_uses_every_export() {
    for body in [
        "export const all = Object.keys(shapes);\n",
        "export const copy = { ...shapes };\n",
        "export const pick = (name: string) => (shapes as Record<string, unknown>)[name];\n",
    ] {
        let source = format!("import * as shapes from './shapes';\n{body}");
        let diagnostics = lint_files(&[PACKAGE, ("index.ts", source.as_str()), SHAPES], "");
        assert_eq!(
            found(&diagnostics, "unused-namespace-exports"),
            none(),
            "{body}: {diagnostics:#?}"
        );
        assert_eq!(found(&diagnostics, "unused-namespace-types"), none(), "{body}");
        assert_eq!(found(&diagnostics, "unused-exports"), none(), "{body}");
    }
}

#[test]
fn a_namespace_re_exported_with_export_star_as_forwards_only_the_members_read_through_it() {
    for barrel in [
        "export * as shapes from './shapes';\n",
        "import * as shapes from './shapes';\nexport { shapes };\n",
    ] {
        let diagnostics = lint_files(
            &[
                PACKAGE,
                (
                    "index.ts",
                    "import { shapes } from './barrel';\nexport const area = shapes.circle(1);\n",
                ),
                ("barrel.ts", barrel),
                SHAPES,
            ],
            "",
        );
        assert_eq!(
            found(&diagnostics, "unused-namespace-exports"),
            ["shapes.ts: square"],
            "{barrel}: {diagnostics:#?}"
        );
        assert_eq!(
            found(&diagnostics, "unused-exports"),
            none(),
            "{barrel}: {diagnostics:#?}"
        );
    }
}

#[test]
fn a_re_exported_namespace_used_as_a_whole_uses_every_export() {
    for barrel in [
        "export * as shapes from './shapes';\n",
        "import * as shapes from './shapes';\nexport { shapes };\n",
    ] {
        let diagnostics = lint_files(
            &[
                PACKAGE,
                (
                    "index.ts",
                    "import { shapes } from './barrel';\nexport const all = Object.values(shapes);\n",
                ),
                ("barrel.ts", barrel),
                SHAPES,
            ],
            "",
        );
        assert_eq!(
            found(&diagnostics, "unused-namespace-exports"),
            none(),
            "{barrel}: {diagnostics:#?}"
        );
        assert_eq!(found(&diagnostics, "unused-exports"), none(), "{barrel}");
    }
}

#[test]
fn an_unused_re_exported_binding_is_reported_by_the_barrel() {
    let diagnostics = lint_files(
        &[
            PACKAGE,
            (
                "index.ts",
                "import { other } from './barrel';\nexport const o = other;\n",
            ),
            (
                "barrel.ts",
                "import * as shapes from './shapes';\nexport { shapes };\nexport const other = 1;\n",
            ),
            SHAPES,
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        ["barrel.ts: shapes", "shapes.ts: circle", "shapes.ts: square"]
            .map(String::from)
            .to_vec(),
        "{diagnostics:#?}"
    );
}

const UTIL: (&str, &str) = (
    "src/util.ts",
    "export const double = (n: number) => n * 2;\nexport const triple = (n: number) => n * 3;\n",
);

fn component_run(component: &str, source: &str) -> Vec<Diagnostic> {
    let main = format!("import App from './{component}';\nexport default App;\n");
    let path = format!("src/{component}");
    lint_files(
        &[
            ("package.json", r#"{"name":"app","private":true,"main":"src/main.ts"}"#),
            ("src/main.ts", main.as_str()),
            (path.as_str(), source),
            UTIL,
        ],
        "",
    )
}

#[test]
fn members_read_in_component_templates_count_as_used() {
    let cases = [
        (
            "App.vue",
            "<script setup lang=\"ts\">\nimport * as util from './util';\n</script>\n<template><p :n=\"util.double(2)\">{{ util.double(3) }}</p></template>\n",
        ),
        (
            "App.svelte",
            "<script>\n  import * as util from './util';\n  let ready = true;\n</script>\n{#if ready}<p>{util.double(1)}</p>{/if}\n",
        ),
        (
            "App.astro",
            "---\nimport * as util from './util';\n---\n<p>{util.double(1)}</p>\n",
        ),
    ];
    for (component, source) in cases {
        let diagnostics = component_run(component, source);
        assert_eq!(
            found(&diagnostics, "unused-namespace-exports"),
            ["src/util.ts: triple"],
            "{component}: {diagnostics:#?}"
        );
        assert_eq!(
            found(&diagnostics, "unused-exports"),
            none(),
            "{component}: {diagnostics:#?}"
        );
        assert_eq!(found(&diagnostics, "unused-files"), none(), "{component}");
    }
}

#[test]
fn a_component_that_does_not_read_the_namespace_in_its_template_reports_it() {
    let diagnostics = component_run(
        "App.vue",
        "<script setup lang=\"ts\">\nimport * as util from './util';\n</script>\n<template><p>util.double</p></template>\n",
    );
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        ["src/util.ts: double", "src/util.ts: triple"],
        "{diagnostics:#?}"
    );
}

#[test]
fn a_namespace_used_as_a_whole_in_a_template_uses_every_export() {
    let diagnostics = component_run(
        "App.vue",
        "<script setup lang=\"ts\">\nimport * as util from './util';\n</script>\n<template><Panel :tools=\"util\" /></template>\n",
    );
    assert_eq!(found(&diagnostics, "unused-exports"), none(), "{diagnostics:#?}");
    assert_eq!(
        found(&diagnostics, "unused-namespace-exports"),
        none(),
        "{diagnostics:#?}"
    );
}

#[test]
fn imports_of_one_script_block_read_by_another_count_as_used() {
    let diagnostics = component_run(
        "App.svelte",
        "<script context=\"module\">\n  import * as util from './util';\n</script>\n<script>\n  export let n = util.double(1);\n</script>\n<p>{n}</p>\n",
    );
    assert_eq!(
        found(&diagnostics, "unused-namespace-exports"),
        ["src/util.ts: triple"],
        "{diagnostics:#?}"
    );
}

#[test]
fn style_blocks_and_src_attributes_import_stylesheets() {
    let manifest = (
        "package.json",
        r#"{"name":"app","private":true,"dependencies":{"normalize.css":"8.0.1","bootstrap":"5.0.0"}}"#,
    );
    let main = ("src/main.ts", "import App from './App.vue';\nexport default App;\n");
    let styled = lint_files(
        &[
            manifest,
            main,
            (
                "src/App.vue",
                "<template><p/></template>\n<style>\n@import 'normalize.css';\n</style>\n<style src=\"bootstrap/dist/css/bootstrap.css\"></style>\n",
            ),
        ],
        "",
    );
    assert_eq!(found(&styled, "unused-dependencies"), none(), "{styled:#?}");

    let plain = lint_files(&[manifest, main, ("src/App.vue", "<template><p/></template>\n")], "");
    assert_eq!(
        found(&plain, "unused-dependencies"),
        ["package.json: bootstrap", "package.json: normalize.css"],
        "{plain:#?}"
    );
}

#[test]
fn diagnostics_in_a_component_point_into_the_original_file() {
    let source = "<template><p/></template>\n<script lang=\"ts\">\nexport const spare = 1;\n</script>\n";
    let diagnostics = lint_files(
        &[
            ("package.json", r#"{"name":"app","private":true,"main":"src/main.ts"}"#),
            ("src/main.ts", "import './Panel.vue';\n"),
            ("src/Panel.vue", source),
        ],
        "",
    );
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.rule.as_str() == "unused-exports")
        .unwrap();
    let span = diagnostic.location.span.unwrap();
    assert_eq!(&source[span.start as usize..span.end as usize], "spare");
    assert_eq!((span.start_line, span.start_col), (3, 14));
}

#[test]
fn docusaurus_virtual_modules_are_neither_unlisted_nor_unresolved() {
    let files = [
        (
            "package.json",
            r#"{"name":"site","private":true,"main":"src/pages/index.js","dependencies":{"@docusaurus/core":"3.0.0","@docusaurus/preset-classic":"3.0.0","react":"18.0.0"}}"#,
        ),
        ("docusaurus.config.js", "module.exports = { presets: ['classic'] };\n"),
        (
            "src/pages/index.js",
            "import Layout from '@theme/Layout';\nimport meta from '@generated/site-metadata';\nimport Link from '@docusaurus/Link';\nimport local from '@site/src/local';\nexport default () => [Layout, meta, Link, local];\n",
        ),
        ("src/local.js", "export default 1;\n"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unresolved-imports"), none(), "{diagnostics:#?}");

    let disabled = lint_files(&files, "[js.plugins]\ndocusaurus = false\n");
    assert!(
        !found(&disabled, "unlisted-dependencies").is_empty(),
        "the control without the plugin reports the virtual modules: {disabled:#?}"
    );
}

#[test]
fn nuxt_virtual_modules_and_aliases_resolve() {
    let files = [
        (
            "package.json",
            r#"{"name":"web","private":true,"dependencies":{"nuxt":"3.0.0","vue":"3.0.0"}}"#,
        ),
        ("nuxt.config.ts", "export default defineNuxtConfig({});\n"),
        ("index.ts", "import './app.vue';\n"),
        (
            "app.vue",
            "<script setup lang=\"ts\">\nimport { ref } from '#imports';\nimport { helper } from '~/utils/helper';\nconst x = ref(helper());\n</script>\n<template><p>{{ x }}</p></template>\n",
        ),
        ("utils/helper.ts", "export const helper = () => 1;\n"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unresolved-imports"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none(), "{diagnostics:#?}");

    let disabled = lint_files(&files, "[js.plugins]\nnuxt = false\n");
    assert!(
        !found(&disabled, "unresolved-imports").is_empty(),
        "the control without the plugin reports `#imports`: {disabled:#?}"
    );
}

#[test]
fn the_node_plugin_keeps_server_js_reachable() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","private":true,"scripts":{"test":"node --test"}}"#,
        ),
        ("server.js", "export const port = 1;\n"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");

    let disabled = lint_files(&files, "[js.plugins]\nnode = false\n");
    assert_eq!(
        found(&disabled, "unused-files"),
        ["server.js: server.js"],
        "{disabled:#?}"
    );
}
