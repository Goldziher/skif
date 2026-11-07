//! Regressions found on a large pnpm monorepo: builtin-named packages, HTML entries of the vitest plugin, referenced
//! `tsconfig` paths, extglob entries, shared config helpers, stylesheet lookups and per-package bundler extensions.
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

const ROOT: &str = "/ws";

/// Lints `memory` with the JS provider and the plugin table, the way the CLI does.
fn lint_memory(memory: MemoryFs, toml: &str) -> Vec<Diagnostic> {
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

fn lint_files(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    lint_memory(memory, toml)
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

const PNPM: &str = r#""packageManager":"pnpm@9.0.0""#;

fn manifest(body: &str) -> String {
    format!(r"{{{body}}}")
}

#[test]
fn trailing_slash_imports_name_npm_packages_not_builtins() {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"main":"index.js","dependencies":{{"buffer":"6.0.3","events":"3.3.0"}}"#
    ));
    let diagnostics = lint_files(
        &[
            ("package.json", &package),
            (
                "index.js",
                "import buf from 'buffer/';\nimport ev from 'events/';\nexport default [buf, ev];\n",
            ),
        ],
        "",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn vitest_adds_index_html_scripts_without_the_vite_plugin() {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"scripts":{{"test":"vitest run"}},"dependencies":{{"lib-a":"1.0.0"}},"devDependencies":{{"vitest":"2.0.0"}}"#
    ));
    let files = [
        ("package.json", package.as_str()),
        (
            "vitest.config.ts",
            "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { include: ['src/**/*.test.ts'] } });\n",
        ),
        (
            "index.html",
            "<html><body><script type=\"module\" src=\"src/boot.js\"></script></body></html>",
        ),
        ("src/boot.js", "import 'lib-a';\n"),
        ("src/a.test.ts", "import { it } from 'vitest';\nit('x', () => {});\n"),
    ];
    let diagnostics = lint_files(&files, "[js.plugins]\nvite = false\n");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
}

fn references_workspace(app_manifest: &str) -> Vec<(&'static str, String)> {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"devDependencies":{{"typescript":"5.6.0"}}"#
    ));
    let host = manifest(r#""name":"host","version":"1.0.0","private":true,"scripts":{"build":"node src/index.js"}"#);
    let icons = manifest(
        r#""name":"@scope/icons","version":"0.0.0","private":true,"type":"module","main":"./dist/index.js","types":"./dist/index.d.ts","exports":{".":{"types":"./dist/index.d.ts","import":"./dist/index.js"}}"#,
    );
    vec![
        ("package.json", root),
        (
            "pnpm-workspace.yaml",
            "packages:\n  - app\n  - host\n  - icons\n".to_owned(),
        ),
        (
            "tsconfig.json",
            r#"{"files":[],"references":[{"path":"./host"}]}"#.to_owned(),
        ),
        ("host/package.json", host),
        (
            "host/tsconfig.json",
            r#"{"compilerOptions":{"composite":true,"paths":{"@scope/icons":["../icons"]}}}"#.to_owned(),
        ),
        ("host/src/index.js", "console.log(1);\n".to_owned()),
        ("icons/package.json", icons),
        ("app/package.json", app_manifest.to_owned()),
        (
            "app/src/index.js",
            "import { Icon } from '@scope/icons';\nconsole.log(Icon);\n".to_owned(),
        ),
    ]
}

fn lint_owned(files: &[(&str, String)]) -> Vec<Diagnostic> {
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
    lint_files(&borrowed, "")
}

#[test]
#[ignore = "the unlisted case still resolves through the referenced paths"]
fn referenced_tsconfig_paths_do_not_become_aliases_of_other_packages() {
    let listed = manifest(
        r#""name":"app","version":"1.0.0","private":true,"scripts":{"build":"node src/index.js"},"dependencies":{"@scope/icons":"workspace:*"}"#,
    );
    let diagnostics = lint_owned(&references_workspace(&listed));
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");

    let unlisted = manifest(r#""name":"app","version":"1.0.0","private":true,"scripts":{"build":"node src/index.js"}"#);
    let diagnostics = lint_owned(&references_workspace(&unlisted));
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["app/src/index.js: @scope/icons"],
        "{diagnostics:#?}"
    );
}

#[test]
fn extglob_groups_in_a_config_entry_are_entries() {
    let package = manifest(&format!(
        r#""name":"lib","version":"1.0.0","private":true,{PNPM},"scripts":{{"build":"tsup"}},"dependencies":{{"dep-a":"1.0.0","dep-b":"1.0.0"}},"devDependencies":{{"tsup":"8.0.0"}}"#
    ));
    let files = [
        ("package.json", package.as_str()),
        (
            "tsup.config.ts",
            "import { defineConfig } from 'tsup';\nexport default defineConfig({ entry: ['src/**/*.(ts|tsx)'], format: ['esm'] });\n",
        ),
        ("src/a.ts", "import 'dep-a';\n"),
        ("src/b.tsx", "import 'dep-b';\n"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
}

#[test]
#[ignore = "the helper's package is not credited yet"]
fn a_shared_config_helper_in_a_sibling_package_credits_the_files_it_names() {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"devDependencies":{{"vitest":"2.0.0","mem-db":"1.0.0"}}"#
    ));
    let cfg = manifest(
        r#""name":"cfg","version":"1.0.0","private":true,"devDependencies":{"vitest":"2.0.0","@scope/env":"workspace:*"}"#,
    );
    let env = manifest(r#""name":"@scope/env","version":"1.0.0","private":true,"main":"./index.js""#);
    let lib = manifest(r#""name":"lib","version":"1.0.0","private":true,"devDependencies":{"vitest":"2.0.0"}"#);
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - cfg\n  - lib\n  - env\n"),
        ("cfg/package.json", cfg.as_str()),
        (
            "cfg/vitest.ts",
            "import path from 'node:path';\nimport { defineConfig } from 'vitest/config';\nconst here = import.meta.dirname;\nexport function defineSharedConfig(options: { db?: boolean } = {}) {\n  return defineConfig({ test: { globalSetup: options.db ? [path.join(here, 'global-setup.ts')] : [] } });\n}\n",
        ),
        (
            "cfg/global-setup.ts",
            "import { MemDb } from 'mem-db';\nimport { env } from '@scope/env';\nexport default async function setup() { return new MemDb(env); }\n",
        ),
        ("env/package.json", env.as_str()),
        ("env/index.js", "export const env = {};\n"),
        ("lib/package.json", lib.as_str()),
        (
            "lib/vitest.config.ts",
            "import { defineSharedConfig } from '../cfg/vitest';\nexport default defineSharedConfig({ db: true });\n",
        ),
        ("lib/src/index.ts", "export const x = 1;\n"),
    ];
    let diagnostics = lint_files(&files, "");
    for rule in ["unused-files", "unused-dependencies", "unused-dev-dependencies"] {
        assert_eq!(found(&diagnostics, rule), none(), "{rule}: {diagnostics:#?}");
    }
}

fn stylesheet_workspace(partial: &str, tsconfig: &str) -> Vec<(String, String)> {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"scripts":{{"build":"node src/index.js"}}"#
    ));
    vec![
        ("package.json".to_owned(), package),
        ("tsconfig.json".to_owned(), tsconfig.to_owned()),
        (
            "src/index.js".to_owned(),
            "import './views/deep/deep.scss';\nimport './index.scss';\n".to_owned(),
        ),
        ("src/index.scss".to_owned(), "@import 'components/mixins';\n".to_owned()),
        (
            "src/views/deep/deep.scss".to_owned(),
            "@import 'components/mixins';\n".to_owned(),
        ),
        (
            format!("src/components/{partial}"),
            "@mixin m { color: red; }\n".to_owned(),
        ),
    ]
}

fn lint_strings(files: &[(String, String)], toml: &str) -> Vec<Diagnostic> {
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    lint_files(&borrowed, toml)
}

#[test]
fn stylesheet_imports_resolve_through_base_url_with_stylesheet_rules() {
    for partial in ["mixins.scss", "_mixins.scss"] {
        let files = stylesheet_workspace(partial, r#"{"compilerOptions":{"baseUrl":"src"}}"#);
        let diagnostics = lint_strings(&files, "");
        assert_eq!(
            found(&diagnostics, "unlisted-dependencies"),
            none(),
            "{partial}: {diagnostics:#?}"
        );
        assert_eq!(
            found(&diagnostics, "unused-files"),
            none(),
            "{partial}: {diagnostics:#?}"
        );
    }
}

#[test]
fn stylesheet_imports_resolve_through_configured_paths() {
    let files = stylesheet_workspace("mixins.scss", "{}");
    let toml = "[js.workspaces.\".\".paths]\n\"components/*\" = [\"./src/components/*\"]\n";
    let diagnostics = lint_strings(&files, toml);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn bundler_extensions_apply_to_the_package_that_declares_them() {
    let root = manifest(&format!(r#""name":"root","private":true,{PNPM}"#));
    let bundled = manifest(
        r#""name":"bundled","version":"1.0.0","private":true,"scripts":{"build":"vite build"},"devDependencies":{"vite":"6.0.0"}"#,
    );
    let other = manifest(r#""name":"other","version":"1.0.0","private":true,"main":"src/index.js""#);
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - bundled\n  - other\n"),
        ("bundled/package.json", bundled.as_str()),
        (
            "bundled/vite.config.js",
            "import { defineConfig } from 'vite';\nexport default defineConfig({ resolve: { extensions: ['.js', '.svg'] } });\n",
        ),
        ("other/package.json", other.as_str()),
        (
            "other/src/index.js",
            "import Logo from './logo';\nexport default Logo;\n",
        ),
        ("other/src/logo.svg", "<svg/>"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        ["other/src/index.js: ./logo"],
        "{diagnostics:#?}"
    );
}

#[test]
fn a_script_path_that_leaves_its_package_is_the_file_it_names() {
    let root = manifest(&format!(
        r#""name":"root","version":"1.0.0","private":true,{PNPM},"devDependencies":{{"lodash":"4.17.21"}}"#
    ));
    let db = manifest(
        r#""name":"db","version":"1.0.0","private":true,"scripts":{"seed":"tsx ../../scripts/seed.ts"},"devDependencies":{"tsx":"4.0.0"}"#,
    );
    let diagnostics = lint_files(
        &[
            ("package.json", &root),
            ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
            ("packages/db/package.json", &db),
            ("scripts/seed.ts", "import { noop } from 'lodash';\nnoop();\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn a_url_into_node_modules_is_a_path_not_a_package() {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"main":"src/index.js""#
    ));
    let diagnostics = lint_files(
        &[
            ("package.json", &package),
            (
                "src/index.js",
                "export const tsx = new URL('../node_modules/.bin/tsx', import.meta.url);\n\
                 export const later = new URL('./node_modules/later', import.meta.url);\n",
            ),
            ("node_modules/.bin/tsx", "#!/usr/bin/env node\n"),
        ],
        "",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}
