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
        (
            "node_modules/.bin/tool",
            "#!/bin/sh\nexec node \"$basedir/../tool/cli.js\" \"$@\"\n",
        ),
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

/// An editor extension declares the editor in `engines` and its client package, nested in it, imports the editor's
/// module: the enclosing package's `engines` provides it as the package's own would.
#[test]
fn a_runtime_in_the_engines_of_an_enclosing_package_provides_its_module() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["ext","ext/client"]}"#,
        ),
        (
            "ext/package.json",
            r#"{"name":"ext","version":"1.0.0","engines":{"vscode":"^1.75.0"},"main":"index.js"}"#,
        ),
        ("ext/index.js", "module.exports = {};\n"),
        (
            "ext/client/package.json",
            r#"{"name":"ext-client","version":"1.0.0","main":"src/extension.js"}"#,
        ),
        (
            "ext/client/src/extension.js",
            "const vscode = require('vscode');\nconst other = require('other-runtime');\nexports.activate = () => [vscode, other];\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["ext/client/src/extension.js: other-runtime"]
    );
}

/// A dependency named like a binary is taken to provide it only while nothing says otherwise: the installed `stripe`
/// SDK has no `bin`, so `stripe listen` runs the CLI, which no dependency provides.
#[test]
fn an_installed_dependency_without_the_binary_does_not_provide_it() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"web","version":"1.0.0","main":"index.js","scripts":{"listen":"stripe listen","serve":"server start"},"dependencies":{"server":"1.0.0","stripe":"9.0.0"}}"#,
        ),
        ("index.js", "module.exports = [require('stripe'), require('server')];\n"),
        (
            "node_modules/stripe/package.json",
            r#"{"name":"stripe","version":"9.0.0","main":"index.js"}"#,
        ),
        ("node_modules/stripe/index.js", "module.exports = {};\n"),
    ]);
    assert_eq!(found(&diagnostics, "unlisted-binaries"), ["package.json: stripe"]);
}

/// A source file that runs a command through `node:child_process` runs its binary as a script would: a listed
/// provider is used, an unlisted binary is reported at the file, and on-demand fetches and system tools are not.
#[test]
fn child_process_commands_run_binaries_of_the_file() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"cowsay":"1.0.0","tsx":"4.0.0"}}"#,
        ),
        (
            "src/index.ts",
            "import { execFileSync, execSync } from 'node:child_process';\n\
             execFileSync('sqlite3', ['db.sqlite']);\nexecSync('tsx scripts/seed.ts');\n\
             execSync('git status');\nexecSync('npx cowsay hi');\nexport const done = true;\n",
        ),
        ("scripts/seed.ts", "export const seed = 1;\n"),
        (
            "src/unreached.ts",
            "import { execSync } from 'node:child_process';\nexecSync('never-run');\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unlisted-binaries"), ["src/index.ts: sqlite3"]);
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
}

/// The commands of a workflow, and of a source file that runs them with `execa`, run binaries of that file.
#[test]
fn binaries_that_workflows_and_execa_calls_run_are_reported_at_the_file() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"scripts/release.mjs","dependencies":{"execa":"9.0.0"},"devDependencies":{"vitest":"3.0.0"}}"#,
        ),
        (
            ".github/workflows/ci.yml",
            "name: ci\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: vitest run && release-tool publish\n      - run: npx playwright test\n",
        ),
        (
            "scripts/release.mjs",
            "import { execa } from 'execa';\nawait execa('changeset', ['publish']);\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-binaries"),
        [
            ".github/workflows/ci.yml: release-tool",
            "scripts/release.mjs: changeset"
        ]
    );
}
