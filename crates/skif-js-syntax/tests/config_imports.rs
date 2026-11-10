//! Static config extraction that follows the modules a config imports.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skif_js_syntax::{ConfigValue, ModuleLoader, UNKNOWN_SPREAD, extract_config_object, extract_config_object_with};

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

/// Two modules whose exports refer to each other give the same values whichever the config reads first.
#[test]
fn a_cycle_between_imports_does_not_depend_on_the_order_of_evaluation() {
    let modules = [
        (
            "/ws/app/a.ts",
            "import { b } from './b';\nexport const a = { name: 'a', other: b };",
        ),
        (
            "/ws/app/b.ts",
            "import { a } from './a';\nexport const b = { name: 'b', other: a };",
        ),
    ];
    let mut a_first = modules.to_vec();
    a_first.push((
        "/ws/app/config.ts",
        "import { a } from './a';\nimport { b } from './b';\nexport default { a, b };",
    ));
    let mut b_first = modules.to_vec();
    b_first.push((
        "/ws/app/config.ts",
        "import { a } from './a';\nimport { b } from './b';\nexport default { b, a };",
    ));
    let a_first = eval(&files(&a_first));
    let b_first = eval(&files(&b_first));
    for key in ["a", "b"] {
        assert_eq!(a_first.get(key), b_first.get(key), "{key}");
    }
    assert_eq!(
        a_first.get("a").unwrap(),
        &obj(vec![
            ("name", s("a")),
            ("other", obj(vec![("name", s("b")), ("other", ConfigValue::Unknown)])),
        ])
    );
}

/// A chain of `count` modules, the config first, each exporting `{ name, next }` of the module after it.
fn chain(count: usize) -> Files {
    let sources: Vec<(String, &'static str)> = (1..count)
        .map(|index| {
            let source = if index + 1 < count {
                format!(
                    "import {{ v as next }} from './m{}';\nexport const v = {{ name: 'm{index}', next }};",
                    index + 1
                )
            } else {
                format!("export const v = {{ name: 'm{index}' }};")
            };
            (format!("/ws/app/m{index}.ts"), &*Box::leak(source.into_boxed_str()))
        })
        .collect();
    let mut entries: Vec<(&str, &'static str)> =
        sources.iter().map(|(path, source)| (path.as_str(), *source)).collect();
    entries.push(("/ws/app/config.ts", "import { v } from './m1';\nexport default v;"));
    files(&entries)
}

/// The modules of a chain whose values are known, following `next` until it is unknown or absent.
fn known_depth(value: &ConfigValue) -> (usize, Option<&ConfigValue>) {
    let mut depth = 0;
    let mut current = value;
    while let ConfigValue::Object(_) = current {
        depth += 1;
        match current.get("next") {
            Some(next) => current = next,
            None => return (depth, None),
        }
    }
    (depth, Some(current))
}

#[test]
fn a_chain_of_imports_reaches_eight_modules() {
    // The config and seven imported modules: all known.
    assert_eq!(known_depth(&eval(&chain(8))), (7, None));
    // The ninth module of a chain is unknown.
    assert_eq!(known_depth(&eval(&chain(9))), (7, Some(&ConfigValue::Unknown)));
}

#[test]
fn many_calls_of_one_imported_helper_count_as_one_module() {
    let calls: Vec<String> = (0..100).map(|index| format!("make('{index}')")).collect();
    let config = format!(
        "import {{ make }} from './make';\nexport default [{}];",
        calls.join(", ")
    );
    let loader = files(&[
        ("/ws/app/config.ts", Box::leak(config.into_boxed_str())),
        ("/ws/app/make.ts", "export const make = (name) => ({ name });"),
    ]);
    let value = eval(&loader);
    let items = value.as_array().unwrap();
    assert_eq!(items.len(), 100);
    assert_eq!(items[99], obj(vec![("name", s("99"))]));
    assert!(!value.has_unknown());
}

/// The shape of a shared vitest config that adds a key only when an option is given, with destructured defaults.
#[test]
fn a_conditional_spread_with_a_destructured_default_is_known() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { createVitestConfig, vitestConfig } from '@fx/base';\n\
             export default [vitestConfig, createVitestConfig({}, { pinCjs: ['zod'] })];",
        ),
        (
            "/ws/base/index.ts",
            "import { defineConfig } from 'vitest/config';\n\
             const retry = (env = process.env) => (Number(env.RETRY) > 0 ? { retry: Number(env.RETRY) } : {});\n\
             export const createVitestConfig = (options = {}, { pinCjs = [] } = {}) =>\n\
               defineConfig({\n\
                 test: { globals: true, ...retry(), ...options },\n\
                 ...(pinCjs.length ? { resolve: { alias: aliases(pinCjs) } } : {}),\n\
               });\n\
             export const vitestConfig = createVitestConfig();",
        ),
    ]);
    let value = eval(&loader);
    let configs = value.as_array().unwrap();
    assert_eq!(
        configs[0],
        obj(vec![(
            "test",
            obj(vec![
                ("globals", ConfigValue::Bool(true)),
                ("retry", ConfigValue::Unknown)
            ])
        )])
    );
    assert_eq!(
        configs[1].get("resolve"),
        Some(&obj(vec![("alias", ConfigValue::Unknown)]))
    );
    assert!(configs[1].get("...").is_none());
}

/// What an imported module computes itself is, to the config importing it, a value taken from elsewhere: its spreads
/// come back as unknown spreads of an imported value.
#[test]
fn the_own_spreads_of_an_imported_module_are_imported_ones() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { shared } from './shared';\nexport default { build: shared };",
        ),
        (
            "/ws/app/shared.ts",
            "const make = () => JSON.parse(process.env.LIB ?? '{}');\n\
             export const shared = { outDir: 'dist', ...make() };",
        ),
    ]);
    assert_eq!(
        eval(&loader),
        obj(vec![(
            "build",
            obj(vec![("outDir", s("dist")), (UNKNOWN_SPREAD, ConfigValue::Unknown)])
        )])
    );
}
