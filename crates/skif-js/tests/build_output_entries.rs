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
