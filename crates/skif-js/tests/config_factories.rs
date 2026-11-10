//! Tool configs built by a factory (`export default createVitestConfig({ setupFiles: ['./config/boot.ts'] })`): a
//! factory of a workspace package is evaluated with the options it is given.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use skif_config::Config;
use skif_core::{FileSystem, IncompletePackage, MemoryFs};
use skif_js::{InputSource, JsAnalysis, PluginSource};
use skif_workspace::Workspace;

// The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
const ROOT: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };

/// The packages left incomplete and the `explain-usage` report of `file`, as JSON, in a workspace of `files`.
fn analyse(files: &[(&str, &str)], file: &str) -> (Vec<IncompletePackage>, Value) {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::default();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(PluginSource::new(config.js.plugins.clone()))];
    let analysis = JsAnalysis::build(&workspace, &config, &fs, &sources, true);
    let report = serde_json::to_value(analysis.explain_usage(Path::new(file), None)).unwrap();
    (analysis.incomplete_packages(), report)
}

/// A workspace whose app package has `vitest_config` as its Vitest config, and a setup module at `config/boot.ts`,
/// which no default entry glob or test convention matches.
fn workspace(vitest_config: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "packages/app/package.json",
            r#"{"name":"@x/app","private":true,"type":"module","main":"src/index.ts","devDependencies":{"@acme/vitest-preset":"1.0.0","@x/vitest-config":"workspace:*","vitest":"3.0.0"}}"#,
        ),
        ("packages/app/vitest.config.ts", vitest_config),
        ("packages/app/src/index.ts", "export const main = 1;\n"),
        ("packages/app/config/boot.ts", "export const boot = 1;\n"),
        (
            "packages/vitest-config/package.json",
            r#"{"name":"@x/vitest-config","private":true,"type":"module","main":"index.js","dependencies":{"vitest":"3.0.0"}}"#,
        ),
        (
            "packages/vitest-config/index.js",
            "import { defineConfig } from 'vitest/config';\n\
             export function createVitestConfig(options = {}) {\n\
               return defineConfig({\n\
                 test: {\n\
                   globals: true,\n\
                   setupFiles: [...(options.setupFiles ?? [])],\n\
                   include: options.include ?? ['src/**/*.test.ts'],\n\
                 },\n\
               });\n\
             }\n",
        ),
    ]
}

/// A factory of a workspace package that builds the config from its options is evaluated: the setup module it is
/// given is an entry and the package's entries are complete.
#[test]
fn a_workspace_factory_config_is_evaluated_with_its_options() {
    let files = workspace(
        "import { createVitestConfig } from '@x/vitest-config';\n\
         export default createVitestConfig({ setupFiles: ['./config/boot.ts'] });\n",
    );
    let (incomplete, boot) = analyse(&files, "packages/app/config/boot.ts");
    assert_eq!(incomplete, Vec::new());
    assert_eq!(boot["in-graph"], true, "{boot}");
}
