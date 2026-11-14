//! Entries that reach their sources through build output: scripts that run compiled tests, and packages that publish
//! what a build config computes.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

// The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
const ROOT: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };

/// Lints `files` with the JS provider and the plugin table, the way the CLI does.
fn lint_files(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("").unwrap();
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

/// The diagnostics of `rule`, as `path: name` with the dependency or the first backticked name of the message.
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

/// A vendored ORM shaped like n8n's `@n8n/typeorm`: `tsc` compiles `src` and `test` to `build/compiled` with no
/// `rootDir`, and mocha runs the compiled test directory.
#[test]
fn compiled_test_directories_map_back_to_source() {
    let manifest = r#"{"name":"orm","version":"1.0.0","private":true,"packageManager":"pnpm@9.0.0",
        "main":"./dist/index.js",
        "scripts":{"test":"tsc && mocha --file ./build/compiled/test/utils/test-setup.js --recursive --timeout 90000 ./build/compiled/test"},
        "devDependencies":{"chai":"4.3.0","mocha":"10.2.0","typescript":"5.6.0"}}"#;
    let diagnostics = lint_files(&[
        ("package.json", manifest),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"outDir":"build/compiled"},"include":["sample","src","test"],"exclude":["build"]}"#,
        ),
        (
            "tsconfig.build.json",
            r#"{"extends":"./tsconfig.json","compilerOptions":{"rootDir":"src","outDir":"dist"},"include":["src"]}"#,
        ),
        ("src/index.ts", "export const connect = () => 1;\n"),
        ("src/orphan.ts", "export const orphan = 1;\n"),
        (
            "test/utils/test-setup.ts",
            "import { use } from 'chai';\nuse(() => {});\n",
        ),
        ("test/utils/test-utils.ts", "export const createConnection = () => 2;\n"),
        (
            "test/functional/repository/find.ts",
            "import { expect } from 'chai';\nimport { connect } from '../../../src/index';\nimport { createConnection } from '../../utils/test-utils';\nexpect(connect()).to.equal(createConnection());\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/orphan.ts: src/orphan.ts"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// A published path inside an `outDir` maps to its source through the `rootDir` inferred from `include`.
#[test]
fn published_paths_map_through_an_inferred_root_dir() {
    let manifest = r#"{"name":"lib","version":"1.0.0","private":true,"packageManager":"pnpm@9.0.0",
        "main":"./build/src/index.js","devDependencies":{"typescript":"5.6.0"}}"#;
    let diagnostics = lint_files(&[
        ("package.json", manifest),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"outDir":"build"},"include":["src/**/*.ts","scripts/*.ts"]}"#,
        ),
        ("src/index.ts", "export { used } from './used';\n"),
        ("src/used.ts", "export const used = 1;\n"),
        ("src/orphan.ts", "export const orphan = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/orphan.ts: src/orphan.ts"],
        "{diagnostics:#?}"
    );
}

/// A package that compiles `src` and `test` to `build` and runs mocha on the compiled test directory, with `extra`
/// mocha flags before it.
fn compiled_mocha(flags: &str) -> Vec<Diagnostic> {
    let manifest = format!(
        r#"{{"name":"lib","version":"1.0.0","private":true,"packageManager":"pnpm@9.0.0",
        "main":"./build/src/index.js",
        "scripts":{{"test":"tsc && mocha {flags} ./build/test"}},
        "devDependencies":{{"mocha":"10.2.0","typescript":"5.6.0"}}}}"#
    );
    lint_files(&[
        ("package.json", &manifest),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"outDir":"build"},"include":["src","test"]}"#,
        ),
        ("src/index.ts", "export const value = 1;\n"),
        (
            "test/index.test.ts",
            "import { value } from '../src/index';\nexport const checked = value;\n",
        ),
        ("test/helpers/unused.ts", "export const helper = 1;\n"),
    ])
}

/// Without `--recursive` mocha reads only the files directly in a directory it is given.
#[test]
fn a_test_directory_without_recursive_reads_only_its_direct_children() {
    let diagnostics = compiled_mocha("");
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["test/helpers/unused.ts: test/helpers/unused.ts"],
        "{diagnostics:#?}"
    );
}

/// With `--recursive` mocha reads every file below a directory it is given.
#[test]
fn a_test_directory_with_recursive_reads_every_file_below_it() {
    let diagnostics = compiled_mocha("--recursive");
    assert_eq!(
        found(&diagnostics, "unused-files"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// A directory that is not build output is read with mocha's `--extension` values, `js`, `cjs` and `mjs` by default.
#[test]
fn a_source_test_directory_reads_the_given_extensions() {
    let lint = |flags: &str| {
        let manifest = format!(
            r#"{{"name":"lib","version":"1.0.0","private":true,"packageManager":"pnpm@9.0.0",
            "main":"./src/index.ts",
            "scripts":{{"test":"mocha --require ts-node/register {flags} test"}},
            "devDependencies":{{"mocha":"10.2.0","ts-node":"10.9.0","typescript":"5.6.0"}}}}"#
        );
        let diagnostics = lint_files(&[
            ("package.json", &manifest),
            ("src/index.ts", "export const value = 1;\n"),
            (
                "test/index.test.ts",
                "import { value } from '../src/index';\nexport const checked = value;\n",
            ),
            ("test/helpers/unused.ts", "export const helper = 1;\n"),
        ]);
        found(&diagnostics, "unused-files")
    };
    assert_eq!(
        lint(""),
        [
            "test/helpers/unused.ts: test/helpers/unused.ts",
            "test/index.test.ts: test/index.test.ts"
        ]
    );
    assert_eq!(
        lint("--extension ts"),
        ["test/helpers/unused.ts: test/helpers/unused.ts"]
    );
    assert_eq!(lint("--extension ts --recursive"), Vec::<String>::new());
}

/// A script that runs a build artefact stands for its source even when the (ignored) artefact is on disk.
#[test]
fn a_built_script_on_disk_does_not_shadow_its_source() {
    let manifest = r#"{"name":"cli","version":"1.0.0","private":true,"packageManager":"pnpm@9.0.0",
        "scripts":{"start":"node build/src/serve.js"},"devDependencies":{"typescript":"5.6.0"}}"#;
    let diagnostics = lint_files(&[
        ("package.json", manifest),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"outDir":"build"},"include":["src","test"]}"#,
        ),
        ("src/serve.ts", "import { run } from './run';\nrun();\n"),
        ("src/run.ts", "export const run = () => 1;\n"),
        ("src/orphan.ts", "export const orphan = 1;\n"),
        (".gitignore", "build\n"),
        ("build/src/serve.js", "console.log(1);\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/orphan.ts: src/orphan.ts"],
        "{diagnostics:#?}"
    );
}

/// A package whose webpack config bundles `lib-esm/entry.js`, the build output of `src/entry.tsx`, with `extra` files.
fn bundled_build_output(extra: &[(&str, &str)]) -> Vec<Diagnostic> {
    let mut files = vec![
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"packageManager":"pnpm@9.0.0",
            "dependencies":{"react":"18.0.0"},"devDependencies":{"typescript":"5.6.0","webpack":"5.0.0"}}"#,
        ),
        (
            "webpack.config.js",
            "const path = require('path');\n\
             module.exports = () => ({ entry: { bundle: path.join(__dirname, 'lib-esm', 'entry.js') } });\n",
        ),
        (
            "src/entry.tsx",
            "import { createElement } from 'react';\nimport { title } from './title';\n\
             export const app = createElement('h1', null, title);\n",
        ),
        ("src/title.ts", "export const title = 'app';\n"),
        ("src/orphan.ts", "export const orphan = 1;\n"),
    ];
    files.extend_from_slice(extra);
    lint_files(&files)
}

/// A bundler entry in a `tsconfig` `outDir` maps back to its source below the `rootDir`.
#[test]
fn a_plugin_entry_in_an_out_dir_maps_back_to_its_source() {
    let diagnostics = bundled_build_output(&[(
        "tsconfig.json",
        r#"{ "compilerOptions": { "outDir": "lib-esm", "rootDir": "src" } }"#,
    )]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/orphan.ts: src/orphan.ts"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// Without a `tsconfig`, a bundler entry in a build output directory maps back to its `src` twin, as a published path
/// does.
#[test]
fn a_plugin_entry_in_a_build_directory_maps_back_to_src() {
    let diagnostics = bundled_build_output(&[]);
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["src/orphan.ts: src/orphan.ts"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}
