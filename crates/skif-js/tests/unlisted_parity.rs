//! Unlisted dependencies and binaries where the corpus runs showed skif and knip disagreeing and skif was wrong:
//! test environment pragmas, `node_modules/.bin` paths, bare imports in plain stylesheets, runtime modules that an
//! enclosing package's `engines` provides, and dependencies whose installed manifest has no such binary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
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

/// `<path>: <dependency>` of every diagnostic of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display(),
                d.dependency.clone().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

fn none() -> Vec<String> {
    Vec::new()
}

/// A `@jest-environment` or `@vitest-environment` pragma loads the environment's package for the test file: it is a
/// use of a listed one and an unlisted dependency when nothing lists it.
#[test]
fn test_environment_pragmas_use_the_environment_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"happy-dom":"15.0.0","vitest":"3.0.0"}}"#,
        ),
        ("src/index.ts", "export const a = 1;\n"),
        (
            "src/dom.test.ts",
            "// @vitest-environment happy-dom\nimport { it } from 'vitest';\nimport { a } from './index';\nit('a', () => a);\n",
        ),
        (
            "src/jsdom.test.ts",
            "/**\n * @vitest-environment jsdom\n */\nimport { it } from 'vitest';\nit('b', () => 1);\n",
        ),
        (
            "src/node.test.ts",
            "// @vitest-environment node\nimport { it } from 'vitest';\nit('c', () => 1);\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["src/jsdom.test.ts: jsdom"]
    );
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
}

/// `require.resolve('.bin/tool')` finds the package manager's link in `node_modules/.bin`, which is no package.
#[test]
fn a_node_modules_bin_path_names_no_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.js","dependencies":{"tool":"1.0.0"}}"#,
        ),
        (
            "src/index.js",
            "const tool = require('tool');\nexports.bin = require.resolve('.bin/tool');\nexports.tool = tool;\n",
        ),
        ("node_modules/.bin/tool", "#!/bin/sh\nexec node \"$basedir/../tool/cli.js\" \"$@\"\n"),
        (
            "node_modules/tool/package.json",
            r#"{"name":"tool","version":"1.0.0","main":"index.js","bin":{"tool":"cli.js"}}"#,
        ),
        ("node_modules/tool/index.js", "module.exports = 1;\n"),
        ("node_modules/tool/cli.js", "#!/usr/bin/env node\n"),
    ]);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
}

/// A bare `@import` in a plain stylesheet names the file beside it when one matches (`postcss-import` and the CSS URL
/// rules look there first), and a package otherwise.
#[test]
fn bare_imports_in_plain_stylesheets_are_looked_up_beside_the_file_first() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"theme","version":"1.0.0","main":"src/index.js","devDependencies":{"tailwindcss":"4.0.0"}}"#,
        ),
        ("src/index.js", "import './css/main.css';\n"),
        (
            "src/css/main.css",
            "@import '_code';\n@import 'social';\n@import 'tailwindcss';\n@import 'missing-theme';\n",
        ),
        ("src/css/_code.css", "code { color: red; }\n"),
        ("src/css/_social.css", ".social { color: blue; }\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["src/css/main.css: missing-theme"]
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
}
