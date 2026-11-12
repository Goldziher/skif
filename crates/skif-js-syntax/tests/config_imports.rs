//! Static config extraction that follows the modules a config imports.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skif_js_syntax::{
    ConfigValue, ModuleLoader, UNKNOWN_SPREAD, extract_config, extract_config_object, extract_config_object_with,
};

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
            // `legacy` has no `ignored` key: the member is `undefined`.
            ("c", ConfigValue::Null),
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

/// A factory of a followed module that builds and returns new objects (a `define…` helper of a package given a
/// literal, a spread of a constant, an undeclared global given a parameter) changes no value of a module when the
/// config calls it: what the call returns and a constant the module computes with it when it is loaded stay exact.
#[test]
fn a_factory_that_builds_new_objects_leaves_the_imported_values_known() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { config, createConfig } from '@fx/base';\n\
             export default [config, createConfig(['a.test.ts'])];",
        ),
        (
            "/ws/base/index.ts",
            "import { defineConfig } from 'vitest/config';\n\
             const shared = { environment: 'node' };\n\
             export const createConfig = (include = ['src/**/*.test.ts']) =>\n\
               defineConfig({ test: { globals: true, include: normalize(include) }, resolve: { ...shared } });\n\
             export const config = createConfig();",
        ),
    ]);
    let expected = obj(vec![
        (
            "test",
            obj(vec![
                ("globals", ConfigValue::Bool(true)),
                ("include", ConfigValue::Unknown),
            ]),
        ),
        ("resolve", obj(vec![("environment", s("node"))])),
    ]);
    assert_eq!(eval(&loader), ConfigValue::Array(vec![expected.clone(), expected]));
}

/// A factory of a followed module that changes an object the module binds (directly, through a local holding it,
/// by giving it to an unknown call or to a function of the module that changes its parameter) may change every value
/// the config imports from a followed module, a constant the factory does not touch included.
#[test]
fn a_factory_that_changes_a_binding_of_its_module_leaves_the_imported_values_open() {
    for factory in [
        "export const createConfig = () => { registry.count += 1; return { test: { globals: true } }; };",
        "export const createConfig = () => {\n\
           const seen = registry;\n  seen.count = 1;\n  return { test: { globals: true } };\n};",
        "export const createConfig = () => { register(registry); return { test: { globals: true } }; };",
        "const touch = (target) => { target.count += 1; return target.count; };\n\
         export const createConfig = () => ({ test: { globals: true }, count: touch(registry) });",
    ] {
        let base = format!("const registry = {{ count: 0 }};\n{factory}\nexport const config = {{ name: 'base' }};");
        let loader = files(&[
            (
                "/ws/app/config.ts",
                "import { config, createConfig } from '@fx/base';\nexport default [config, createConfig()];",
            ),
            ("/ws/base/index.ts", &*Box::leak(base.into_boxed_str())),
        ]);
        let value = eval(&loader);
        assert_eq!(
            value.as_array().unwrap()[0],
            obj(vec![("name", s("base")), (UNKNOWN_SPREAD, ConfigValue::Unknown)]),
            "{factory}"
        );
    }
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

/// A shared config spread into a config and a member of it spread into a key the config replaces
/// (`plugins: [...base.plugins, 'x']`) keep what the shared config names; an unknown spread after a key leaves the key
/// unknown, and a key a known object does not have is `undefined`.
#[test]
fn a_member_of_an_imported_config_is_its_value() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import base from '@fx/base';\nimport { open } from './open';\n\
             export default { ...base, plugins: [...base.plugins, 'stylelint-own'], rules: { ...base.rules }, \
             a: open.a, b: open.b, c: base.missing ?? 'fallback' };",
        ),
        (
            "/ws/base/index.ts",
            "export default { customSyntax: 'postcss-scss', plugins: ['stylelint-scss'], rules: { 'scss/at-rule-no-unknown': true } };",
        ),
        (
            "/ws/app/open.ts",
            "export const open = { a: 1, ...JSON.parse(process.env.X ?? '{}') };",
        ),
    ]);
    assert_eq!(
        eval(&loader),
        obj(vec![
            ("customSyntax", s("postcss-scss")),
            ("plugins", strings(&["stylelint-scss"])),
            ("rules", obj(vec![("scss/at-rule-no-unknown", ConfigValue::Bool(true))])),
            ("plugins", strings(&["stylelint-scss", "stylelint-own"])),
            ("rules", obj(vec![("scss/at-rule-no-unknown", ConfigValue::Bool(true))])),
            ("a", ConfigValue::Unknown),
            ("b", ConfigValue::Unknown),
            ("c", s("fallback")),
        ])
    );
}

/// A config factory of a workspace package is called with the arguments the config gives it: what the factory reads
/// of its options, with defaults for those not given, is known, called directly or wrapped in `defineConfig`.
#[test]
fn a_workspace_factory_is_called_with_the_options_it_is_given() {
    let factory = "import { defineConfig } from 'vitest/config';\n\
         export function createVitestConfig(options = {}) {\n\
           return defineConfig({ test: { globals: true, setupFiles: [...(options.setupFiles ?? []), './shared.ts'], \
         include: options.include ?? ['src/**/*.test.ts'] } });\n\
         }\n\
         export const createBase = (options) => ({ test: { setupFiles: options.setupFiles, environment: options.environment } });";
    let expected = obj(vec![(
        "test",
        obj(vec![
            ("globals", ConfigValue::Bool(true)),
            ("setupFiles", strings(&["./setup.ts", "./shared.ts"])),
            ("include", strings(&["src/**/*.test.ts"])),
        ]),
    )]);
    let direct = files(&[
        (
            "/ws/app/config.ts",
            "import { createVitestConfig } from '@fx/base';\n\
             export default createVitestConfig({ setupFiles: ['./setup.ts'] });",
        ),
        ("/ws/base/index.ts", factory),
    ]);
    assert_eq!(eval(&direct), expected);
    let wrapped = files(&[
        (
            "/ws/app/config.ts",
            "import { defineConfig } from 'vitest/config';\nimport { createBase } from '@fx/base';\n\
             const options = { setupFiles: ['./setup.ts'], environment: 'jsdom' };\n\
             export default defineConfig(createBase(options));",
        ),
        ("/ws/base/index.ts", factory),
    ]);
    assert_eq!(
        eval(&wrapped),
        obj(vec![(
            "test",
            obj(vec![
                ("setupFiles", strings(&["./setup.ts"])),
                ("environment", s("jsdom"))
            ]),
        )])
    );
}

/// A config exporting the result of a call that cannot be followed (a factory of an installed package) is unknown, and
/// keeps the arguments of the call, looked through `defineConfig`, `await` and a constant; a config that is known, or
/// unknown for another reason, keeps none.
#[test]
fn the_arguments_of_an_exported_call_that_cannot_be_followed_are_kept() {
    let loader = files(&[]);
    let extract = |source: &str| {
        let path = Path::new("/ws/app/vitest.config.ts");
        let found = extract_config(source, path, Some(&loader as &dyn ModuleLoader)).expect("config is understood");
        let without_loader = extract_config(source, path, None).expect("config is understood");
        assert_eq!(found, without_loader, "{source}");
        (found.value, found.call_arguments)
    };
    let options = obj(vec![("setupFiles", strings(&["./config/boot.ts"]))]);
    for source in [
        "import { createVitestConfig } from '@acme/vitest-preset';\n\
         export default createVitestConfig({ setupFiles: ['./config/boot.ts'] });",
        "import { defineConfig } from 'vitest/config';\nimport { createBase } from '@acme/vitest-preset';\n\
         const options = { setupFiles: ['./config/boot.ts'] };\n\
         export default defineConfig(createBase(options));",
        "import { createVitestConfig } from '@acme/vitest-preset';\n\
         const config = await createVitestConfig({ setupFiles: ['./config/boot.ts'] });\nexport default config;",
    ] {
        assert_eq!(
            extract(source),
            (ConfigValue::Unknown, vec![options.clone()]),
            "{source}"
        );
    }
    let known = "export default { test: { setupFiles: ['./config/boot.ts'] } };";
    assert_eq!(
        extract(known),
        (
            obj(vec![(
                "test",
                obj(vec![("setupFiles", strings(&["./config/boot.ts"]))])
            )]),
            Vec::new()
        )
    );
    let unknown = "export default JSON.parse(process.env.CONFIG);";
    assert_eq!(
        extract(unknown).1,
        vec![ConfigValue::Unknown],
        "the argument of any unknown call is kept"
    );
    assert_eq!(extract("export default config;"), (ConfigValue::Unknown, Vec::new()));
}

/// A read that is not definite in an imported module stays so when the module exports it by name, directly or
/// through a re-export: a fallback in the config is credited too.
#[test]
fn an_exported_read_that_is_not_definite_stays_so() {
    for specifier in ["./shared", "./index"] {
        let config: &'static str = if specifier == "./shared" {
            "import { files } from './shared';\n\
             export default { test: { setupFiles: files ?? ['./local.ts'] } };"
        } else {
            "import { files } from './index';\n\
             export default { test: { setupFiles: files ?? ['./local.ts'] } };"
        };
        let loader = files(&[
            ("/ws/app/config.ts", config),
            (
                "/ws/app/shared.ts",
                "const base = { ...(process.env.CI ? { setupFiles: ['./ci.ts'] } : {}) };\n\
                 export const files = base.setupFiles;",
            ),
            ("/ws/app/index.ts", "export { files } from './shared';"),
        ]);
        assert_eq!(
            eval(&loader),
            obj(vec![(
                "test",
                obj(vec![("setupFiles", strings(&["./ci.ts", "./local.ts"]))])
            )]),
            "{specifier}"
        );
    }
}

/// The value at `path` below `value`, an unknown spread after a key standing for any value of it.
fn read_at<'a>(value: &'a ConfigValue, path: &[&str]) -> &'a ConfigValue {
    static UNKNOWN: ConfigValue = ConfigValue::Unknown;
    path.iter().fold(value, |current, key| match current {
        ConfigValue::Object(entries) => entries
            .iter()
            .rev()
            .find(|(name, _)| name == key || ConfigValue::is_spread_key(name))
            .map_or(&UNKNOWN, |(name, field)| {
                if ConfigValue::is_spread_key(name) {
                    &UNKNOWN
                } else {
                    field
                }
            }),
        _ => &UNKNOWN,
    })
}

/// An imported config the module changes (a write through it, a call given it, a write through a local holding it, in
/// a function or at the top level) is read as changed, not as the imported module computes it: the key written is
/// unknown, and the rest of the config stays known where the change reaches that key only.
#[test]
fn a_change_of_an_imported_value_is_applied_where_it_is_read() {
    for (config, only_the_key) in [
        (
            "import base from './base';\nbase.build.outDir = 'out';\nexport default base;",
            true,
        ),
        (
            "import base from './base';\nwithPlugins(base);\nexport default base;",
            false,
        ),
        (
            "import base from './base';\nexport default () => { base.build.outDir = 'out'; return base; };",
            true,
        ),
        (
            "import base from './base';\nexport default () => { withPlugins(base); return base; };",
            false,
        ),
        (
            "import base from './base';\nconst local = base;\nlocal.build.outDir = 'out';\nexport default base;",
            true,
        ),
        (
            "import * as shared from './base';\nshared.default.build.outDir = 'out';\nexport default shared.default;",
            true,
        ),
    ] {
        let loader = files(&[
            ("/ws/app/config.ts", config),
            (
                "/ws/app/base.ts",
                "export default { root: 'src', build: { outDir: 'dist' } };",
            ),
        ]);
        let value = eval(&loader);
        assert_eq!(
            read_at(&value, &["build", "outDir"]),
            &ConfigValue::Unknown,
            "{config}: {value:?}"
        );
        let root = if only_the_key { s("src") } else { ConfigValue::Unknown };
        assert_eq!(read_at(&value, &["root"]), &root, "{config}: {value:?}");
    }
}

/// A function of a workspace package the loader follows may return the package's own object (`self: () => config`), so
/// a write through what it returns, also when it is called apart from its object (`(0, base.self)()`), reaches the
/// imported value; the call alone changes nothing.
#[test]
fn a_write_through_what_a_method_of_a_followed_package_returns_reaches_it() {
    for (write, out_dir) in [("same.build.outDir = 'out';", ConfigValue::Unknown), ("", s("dist"))] {
        let config = format!("import base from '@fx/base';\nconst same = (0, base.self)();\n{write}\nexport default base;");
        let loader = files(&[
            ("/ws/app/config.ts", &*Box::leak(config.into_boxed_str())),
            (
                "/ws/base/index.ts",
                "const config = { build: { outDir: 'dist' }, self: () => config };\nexport default config;",
            ),
        ]);
        let value = eval(&loader);
        assert_eq!(read_at(&value, &["build", "outDir"]), &out_dir, "{write}: {value:?}");
    }
}

/// The config that imports `name` from `./shared` with the text `shared`, runs `statement` and exports the plugins of
/// `base` of `./base`, with a `name` of its own.
fn shared_config(shared: &'static str, name: &str, statement: &str) -> ConfigValue {
    let config = format!(
        "import {{ defineConfig }} from 'vite';\nimport {{ base }} from './base';\nimport {{ {name} }} from './shared';\n\
         {statement}\nexport default defineConfig({{ name: 'app', plugins: base.plugins }});"
    );
    let loader = files(&[
        ("/ws/app/config.ts", &*Box::leak(config.into_boxed_str())),
        ("/ws/app/base.ts", "export const base = { plugins: [] };"),
        ("/ws/app/shared.ts", shared),
        (
            "/ws/app/getter.ts",
            "import { base } from './base';\nexport const getBase = () => base;",
        ),
    ]);
    eval(&loader)
}

/// What a function of a followed module returns may be a value of a module (`getBase = () => base`): a change through
/// it, in the config or in a function of another followed module the config calls, reaches the imported values, and a
/// call whose result nothing changes leaves them known.
#[test]
fn a_change_through_what_an_imported_function_returns_reaches_the_imported_values() {
    let getter = "import { base } from './base';\nexport const getBase = () => base;";
    for (shared, name, statement) in [
        (getter, "getBase", "getBase().plugins.push('x');"),
        (getter, "getBase", "const cfg = getBase();\ncfg.plugins.push('x');"),
        (
            "import { getBase } from './getter';\nexport function tweak() { getBase().plugins.push('x'); }",
            "tweak",
            "tweak();",
        ),
    ] {
        let value = shared_config(shared, name, statement);
        assert_eq!(
            read_at(&value, &["plugins"]),
            &ConfigValue::Unknown,
            "{shared}\n{statement}: {value:?}"
        );
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{shared}\n{statement}: {value:?}");
    }
    let value = shared_config(getter, "getBase", "const seen = getBase();");
    assert_eq!(read_at(&value, &["plugins"]), &strings(&[]), "{value:?}");
}

/// A function of a followed module may change a value of a module through what it keeps for every call: a key of the
/// function object (`f.cache = base`), what `bind` gives it (`add.bind(null, base)`), and what a generator or an
/// `async` function was given by the call that made it, whose code runs later. Calling it leaves the imported values
/// partly unknown; one that only reads what it keeps leaves them known.
#[test]
fn a_function_of_a_followed_module_that_changes_what_it_keeps_changes_the_imported_values() {
    for (shared, name, statement) in [
        (
            "import { base } from './base';\nfunction f() {}\nf.cache = base;\n\
             export function addPlugin(p) { f.cache.plugins.push(p); }",
            "addPlugin",
            "addPlugin('x');",
        ),
        (
            "import { base } from './base';\nfunction add(c, p) { c.plugins.push(p); }\n\
             export const addPlugin = add.bind(null, base);",
            "addPlugin",
            "addPlugin('x');",
        ),
        (
            "import { base } from './base';\nfunction* steps(c) { yield; c.plugins.push('x'); }\n\
             const it = steps(base);\nexport const next = it.next.bind(it);",
            "next",
            "next();\nnext();",
        ),
        (
            "import { base } from './base';\nasync function run(c) { await null; c.plugins.push('x'); }\n\
             const pending = run(base);\nexport const settle = () => pending;",
            "settle",
            "await settle();",
        ),
    ] {
        let value = shared_config(shared, name, statement);
        assert_eq!(
            read_at(&value, &["plugins"]),
            &ConfigValue::Unknown,
            "{shared}\n{statement}: {value:?}"
        );
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{shared}\n{statement}: {value:?}");
    }
    for shared in [
        "import { base } from './base';\nfunction f() {}\nf.cache = base;\n\
         export function addPlugin(p) { return [...f.cache.plugins, p]; }",
        "import { base } from './base';\nfunction add(c, p) { return [...c.plugins, p]; }\n\
         export const addPlugin = add.bind(null, base);",
    ] {
        let value = shared_config(shared, "addPlugin", "addPlugin('x');");
        assert_eq!(read_at(&value, &["plugins"]), &strings(&[]), "{shared}: {value:?}");
    }
}

/// A `define…` helper of a workspace package the loader follows is code of the project, which may change what it is
/// given: the config it was given is partly unknown after the call.
#[test]
fn a_define_helper_of_a_followed_package_may_change_what_it_is_given() {
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import { defineConfig } from '@fx/base';\nconst base = { build: { outDir: 'dist' } };\n\
             defineConfig(base);\nexport default base;",
        ),
        (
            "/ws/base/index.ts",
            "export function defineConfig(c) { c.build.outDir = 'out'; return c; }",
        ),
    ]);
    let value = eval(&loader);
    assert_eq!(
        read_at(&value, &["build", "outDir"]),
        &ConfigValue::Unknown,
        "{value:?}"
    );
}

/// Bindings of one module, through a default, a named or a namespace import, are the same exports: a change through
/// one reaches the others.
#[test]
fn a_change_through_one_import_of_a_module_reaches_its_other_imports() {
    for (config, base) in [
        (
            "import base from './base';\nimport * as ns from './base';\n\
             ns.default.build.outDir = 'out';\nexport default base;",
            "export default { build: { outDir: 'dist' } };",
        ),
        (
            "import base from './base';\nimport * as ns from './base';\n\
             base.build.outDir = 'out';\nexport default ns.default;",
            "export default { build: { outDir: 'dist' } };",
        ),
        (
            "import { config } from './base';\nimport * as ns from './base';\n\
             ns.config.build.outDir = 'out';\nexport default config;",
            "export const config = { build: { outDir: 'dist' } };",
        ),
        (
            "import base, { config } from './base';\nconfig.build.outDir = 'out';\nexport default base;",
            "export const config = { build: { outDir: 'dist' } };\nexport default config;",
        ),
    ] {
        let loader = files(&[("/ws/app/config.ts", config), ("/ws/app/base.ts", base)]);
        let value = eval(&loader);
        assert_eq!(
            read_at(&value, &["build", "outDir"]),
            &ConfigValue::Unknown,
            "{config}: {value:?}"
        );
    }
}

/// Calling a function of a followed module that changes a value of a module (an import of its own, a binding it
/// exports), or importing one for its effects, may change every value the config imports from a followed module; a
/// module whose functions change nothing of a module leaves them known.
#[test]
fn a_followed_module_that_changes_an_import_reaches_the_values_the_config_imports() {
    let base = "export default { root: 'src', build: { outDir: 'dist' } };";
    let tweaking = "import base from './base';\nexport function tweak() { base.build.outDir = 'out'; }";
    for (config, tweak) in [
        (
            "import base from './base';\nimport { tweak } from './tweak';\ntweak();\nexport default base;",
            tweaking,
        ),
        (
            "import base from './base';\nimport * as tools from './tweak';\ntools.tweak();\nexport default base;",
            tweaking,
        ),
        (
            "import base from './base';\nimport { tweak } from './tweak';\n\
             export default () => { tweak(); return base; };",
            tweaking,
        ),
        (
            "import base from './base';\nimport { tweak } from './tweak';\nexport default { ...base, x: tweak() };",
            tweaking,
        ),
        (
            "import base from './base';\nimport { tweak } from './tweak';\nexport default { x: tweak(), ...base };",
            tweaking,
        ),
        (
            "import base from './base';\nimport { tweak } from './tweak';\ntweak();\nexport default base;",
            "import { apply } from './apply';\nexport function tweak() { apply(); }",
        ),
        (
            "import base from './base';\nimport './tweak';\nexport default base;",
            "import base from './base';\nbase.build.outDir = 'out';",
        ),
        (
            "const base = require('./base');\nrequire('./tweak');\nmodule.exports = base;",
            "const base = require('./base');\nbase.build.outDir = 'out';",
        ),
    ] {
        let loader = files(&[
            ("/ws/app/config.ts", config),
            ("/ws/app/base.ts", base),
            ("/ws/app/tweak.ts", tweak),
            (
                "/ws/app/apply.ts",
                "import base from './base';\nexport function apply() { base.build.outDir = 'out'; }",
            ),
        ]);
        let value = eval(&loader);
        assert_eq!(
            read_at(&value, &["build", "outDir"]),
            &ConfigValue::Unknown,
            "{config}\n{tweak}: {value:?}"
        );
    }
    let pure = files(&[
        (
            "/ws/app/config.ts",
            "import base from './base';\nimport { name } from './name';\nexport default { ...base, name: name() };",
        ),
        ("/ws/app/base.ts", base),
        ("/ws/app/name.ts", "export function name() { return 'app'; }"),
    ]);
    let value = eval(&pure);
    assert_eq!(read_at(&value, &["build", "outDir"]), &s("dist"), "{value:?}");
    assert_eq!(read_at(&value, &["name"]), &s("app"), "{value:?}");
}
