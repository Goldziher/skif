//! Static config extraction that follows the modules a config imports.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skif_js_syntax::{ConfigValue, ModuleLoader, extract_config_object, extract_config_object_with};

/// Modules by absolute path; relative specifiers resolve with a `.ts` extension, `@fx/base` to `/ws/base/index.ts`.
struct Files(BTreeMap<PathBuf, &'static str>);

impl ModuleLoader for Files {
    fn load(&self, from: &Path, specifier: &str) -> Option<(PathBuf, String)> {
        let path = if let Some(rest) = specifier.strip_prefix("./") {
            from.parent()?.join(rest)
        } else if let Some(rest) = specifier.strip_prefix("../") {
            from.parent()?.parent()?.join(rest)
        } else if specifier == "@fx/base" {
            PathBuf::from("/ws/base/index")
        } else {
            return None;
        };
        let path = if path.extension().is_some() {
            path
        } else {
            path.with_extension("ts")
        };
        self.0.get(&path).map(|source| (path, (*source).to_owned()))
    }
}

fn files(entries: &[(&str, &'static str)]) -> Files {
    Files(
        entries
            .iter()
            .map(|(path, source)| (PathBuf::from(path), *source))
            .collect(),
    )
}

fn eval(loader: &Files) -> ConfigValue {
    let path = Path::new("/ws/app/config.ts");
    let source = loader.0.get(path).expect("the config");
    extract_config_object_with(source, path, loader).expect("config is understood")
}

fn s(v: &str) -> ConfigValue {
    ConfigValue::String(v.to_owned())
}

fn strings(items: &[&str]) -> ConfigValue {
    ConfigValue::Array(items.iter().map(|item| s(item)).collect())
}

fn obj(entries: Vec<(&str, ConfigValue)>) -> ConfigValue {
    ConfigValue::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

#[test]
fn a_relative_default_import_is_evaluated() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { mergeConfig } from 'vitest/config';\nimport base from './base';\n\
             export default mergeConfig(base, { test: { environment: 'node' } });",
        ),
        (
            "/ws/app/base.ts",
            "import { defineConfig } from 'vitest/config';\n\
             export default defineConfig({ test: { include: ['tests/**/*.it.ts'] } });",
        ),
    ]);
    assert_eq!(
        eval(&loader),
        obj(vec![(
            "test",
            obj(vec![
                ("include", strings(&["tests/**/*.it.ts"])),
                ("environment", s("node"))
            ])
        )])
    );
}

#[test]
fn imports_are_unknown_without_a_loader() {
    let value = extract_config_object(
        "import base from './base';\nexport default { ...base, a: 1 };",
        Path::new("/ws/app/config.ts"),
    )
    .unwrap();
    assert_eq!(
        value,
        obj(vec![("...", ConfigValue::Unknown), ("a", ConfigValue::Number(1.0))])
    );
}

#[test]
fn a_package_export_is_evaluated_through_reexports_and_calls() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { defineConfig, mergeConfig } from 'vite';\nimport { vitestConfig } from '@fx/base';\n\
             export default mergeConfig(defineConfig({ build: { outDir: 'dist' } }), vitestConfig);",
        ),
        (
            "/ws/base/index.ts",
            "export * from './frontend';\nexport { setup as setupFiles } from './setup';",
        ),
        (
            "/ws/base/frontend.ts",
            "import { defineConfig } from 'vitest/config';\nimport { setupFiles } from './index';\n\
             export const create = (options = {}) => defineConfig({ test: { setupFiles, ...options } });\n\
             export const vitestConfig = create({ globals: true });",
        ),
        ("/ws/base/setup.ts", "export const setup = ['./src/setup.ts'];"),
    ]);
    assert_eq!(
        eval(&loader),
        obj(vec![
            ("build", obj(vec![("outDir", s("dist"))])),
            (
                "test",
                obj(vec![
                    ("setupFiles", strings(&["./src/setup.ts"])),
                    ("globals", ConfigValue::Bool(true)),
                ])
            ),
        ])
    );
}

#[test]
fn namespace_members_required_modules_and_called_functions_are_evaluated() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import * as shared from './shared';\nimport { make } from './shared';\nconst legacy = require('./legacy');\n\
             export default { a: shared.include, b: make('x'), c: legacy.ignored, d: legacy };",
        ),
        (
            "/ws/app/shared.ts",
            "export const include = ['src/**'];\nexport function make(name) { return { name }; }",
        ),
        ("/ws/app/legacy.ts", "module.exports = { files: ['a.js'] };"),
    ]);
    assert_eq!(
        eval(&loader),
        obj(vec![
            ("a", strings(&["src/**"])),
            ("b", obj(vec![("name", s("x"))])),
            ("c", ConfigValue::Unknown),
            ("d", obj(vec![("files", strings(&["a.js"]))])),
        ])
    );
}

#[test]
fn paths_an_imported_module_computes_are_relative_to_the_config() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { alias } from '../shared/aliases';\nexport default { alias, here: __dirname };",
        ),
        (
            "/ws/shared/aliases.ts",
            "import { resolve } from 'path';\n\
             export const alias = { stub: resolve(__dirname, 'stub.ts'), url: new URL('./b.ts', import.meta.url).pathname };",
        ),
    ]);
    assert_eq!(
        eval(&loader),
        obj(vec![
            (
                "alias",
                obj(vec![("stub", s("../shared/stub.ts")), ("url", s("../shared/b.ts"))])
            ),
            ("here", s(".")),
        ])
    );
}

#[test]
fn a_module_that_imports_the_config_back_is_unknown_there() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { shared } from './shared';\nexport const own = ['x'];\nexport default shared;",
        ),
        (
            "/ws/app/shared.ts",
            "import config, { own } from './config';\nimport { mergeConfig } from 'vite';\n\
             export const shared = mergeConfig(config, { test: { include: own } });",
        ),
    ]);
    assert_eq!(
        eval(&loader),
        obj(vec![
            ("...", ConfigValue::Unknown),
            ("test", obj(vec![("include", strings(&["x"]))])),
        ])
    );
}

#[test]
fn a_type_only_import_binds_no_value() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import type { base } from './base';\nexport default { a: base };",
        ),
        ("/ws/app/base.ts", "export const base = 1;"),
    ]);
    assert_eq!(eval(&loader), obj(vec![("a", ConfigValue::Unknown)]));
}

#[test]
fn a_deep_merge_keeps_the_unknown_side() {
    let value = extract_config_object(
        "import { mergeConfig } from 'vite';\n\
         export default mergeConfig({ test: unknown(), list: other() }, { test: { include: ['a'] }, list: ['b'] });",
        Path::new("config.ts"),
    )
    .unwrap();
    assert_eq!(
        value,
        obj(vec![
            (
                "test",
                obj(vec![("include", strings(&["a"])), ("...", ConfigValue::Unknown)])
            ),
            ("list", ConfigValue::Array(vec![s("b"), ConfigValue::Unknown])),
        ])
    );
}

#[test]
fn eslint_flat_config_combinators_list_all_their_arguments() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { defineConfig } from 'eslint/config';\nimport { frontend } from '@fx/base';\n\
             export default defineConfig(frontend, { files: ['src/**'] }, plugin.configs.recommended);",
        ),
        (
            "/ws/base/index.ts",
            "import tseslint from 'typescript-eslint';\n\
             export const frontend = tseslint.config({ ignores: ['dist'] }, [{ rules: {} }], ...[{ files: ['*.ts'] }]);",
        ),
    ]);
    assert_eq!(
        eval(&loader),
        ConfigValue::Array(vec![
            obj(vec![("ignores", strings(&["dist"]))]),
            obj(vec![("rules", obj(vec![]))]),
            obj(vec![("files", strings(&["*.ts"]))]),
            obj(vec![("files", strings(&["src/**"]))]),
            ConfigValue::Unknown,
        ])
    );
}
