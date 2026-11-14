//! Static config extraction that follows the modules a config imports.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use skif_js_syntax::{
    ConfigValue, ModuleLoader, UNKNOWN_SPREAD, extract_config, extract_config_object, extract_config_object_with,
};

/// Modules by absolute path; relative specifiers resolve with a `.ts` extension, `@fx/base` to `/ws/base/index.ts`, and
/// only scripts are loaded: a declaration file, JSON, a stylesheet or an asset holds no code. Any other bare specifier
/// names an installed package, but for the `@app/` scope, an alias of a project directory the loader does not resolve.
struct Files(BTreeMap<PathBuf, &'static str>);

/// The path a relative specifier names from `from`, as written.
fn relative(from: &Path, specifier: &str) -> Option<PathBuf> {
    if let Some(rest) = specifier.strip_prefix("./") {
        Some(from.parent()?.join(rest))
    } else if let Some(rest) = specifier.strip_prefix("../") {
        Some(from.parent()?.parent()?.join(rest))
    } else {
        None
    }
}

/// Whether `path` has the extension of a module the loader follows.
fn is_script(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "ts" || extension == "js")
}

impl ModuleLoader for Files {
    fn load(&self, from: &Path, specifier: &str) -> Option<(PathBuf, String)> {
        let path = match relative(from, specifier) {
            Some(path) => path,
            None if specifier == "@fx/base" => PathBuf::from("/ws/base/index"),
            None => return None,
        };
        let path = if path.extension().is_some() {
            path
        } else {
            path.with_extension("ts")
        };
        if !is_script(&path) {
            return None;
        }
        self.0.get(&path).map(|source| (path, (*source).to_owned()))
    }

    fn is_installed(&self, _from: &Path, specifier: &str) -> bool {
        !specifier.starts_with(['.', '/']) && !specifier.starts_with("@app/")
    }

    /// A relative specifier of a declaration file beside it (`./types` for `types.d.ts`), or of a file the map holds
    /// that is not a script.
    fn holds_no_code(&self, from: &Path, specifier: &str) -> bool {
        relative(from, specifier).is_some_and(|path| {
            self.0.contains_key(&path.with_extension("d.ts")) || (self.0.contains_key(&path) && !is_script(&path))
        })
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
        let config =
            format!("import base from '@fx/base';\nconst same = (0, base.self)();\n{write}\nexport default base;");
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
        assert_eq!(
            read_at(&value, &["name"]),
            &s("app"),
            "{shared}\n{statement}: {value:?}"
        );
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
        assert_eq!(
            read_at(&value, &["name"]),
            &s("app"),
            "{shared}\n{statement}: {value:?}"
        );
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

/// What `this` or `super` is in a function of a followed module depends on how it is called (`add.bind(base)`), so a
/// write through it changes a value of a module: calling the function leaves the imported values partly unknown, and
/// one that only reads through `this` leaves them known.
#[test]
fn a_write_through_this_in_a_function_of_a_followed_module_changes_the_imported_values() {
    for shared in [
        "import { base } from './base';\nfunction add(p) { this.plugins.push(p); }\n\
         export const addPlugin = add.bind(base);",
        "import { base } from './base';\nconst parent = { list: base.plugins };\n\
         const child = { __proto__: parent, add(p) { super.list.push(p); } };\n\
         export const addPlugin = child.add.bind(child);",
    ] {
        let value = shared_config(shared, "addPlugin", "addPlugin('x');");
        assert_eq!(
            read_at(&value, &["plugins"]),
            &ConfigValue::Unknown,
            "{shared}: {value:?}"
        );
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{shared}: {value:?}");
    }
    let reading = "import { base } from './base';\nfunction copy() { return [...this.plugins]; }\n\
                   export const addPlugin = copy.bind(base);";
    let value = shared_config(reading, "addPlugin", "addPlugin('x');");
    assert_eq!(read_at(&value, &["plugins"]), &strings(&[]), "{value:?}");
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

/// A `define…` helper or a merge of a bare specifier the loader neither loads nor knows as an installed package (an alias
/// of a project directory, `@app/tools`) may do anything, as one of a module of the project may: the config is unknown.
/// One of an installed package (`vite`) is the value it is given, or the merge of what it is given.
#[test]
fn a_define_helper_of_an_alias_the_loader_does_not_follow_is_unknown() {
    let number = |n: f64| ConfigValue::Number(n);
    for (call, known) in [
        ("defineConfig({ a: 1 })", obj(vec![("a", number(1.0))])),
        (
            "mergeConfig({ a: 1 }, { b: 2 })",
            obj(vec![("a", number(1.0)), ("b", number(2.0))]),
        ),
    ] {
        for (specifier, expected) in [("@app/tools", ConfigValue::Unknown), ("vite", known)] {
            let helper = call.split('(').next().unwrap();
            let config = format!("import {{ {helper} }} from '{specifier}';\nexport default {call};");
            let loader = files(&[("/ws/app/config.ts", &*Box::leak(config.into_boxed_str()))]);
            assert_eq!(eval(&loader), expected, "{specifier}: {call}");
        }
    }
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
/// exports, also one it re-exports from a followed module or from one that is neither followed nor installed), or
/// importing one for its effects, may change every value the config imports from a followed module; a module whose
/// functions change nothing of a module, its re-exports included, leaves them known.
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
        (
            "import base from './base';\nimport { tweak } from './tools';\ntweak();\nexport default base;",
            tweaking,
        ),
        (
            "import base from './base';\nimport { tweak } from './barrel';\ntweak();\nexport default base;",
            tweaking,
        ),
        (
            "import base from './base';\nimport { tweak } from './aliased';\ntweak();\nexport default base;",
            tweaking,
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
            ("/ws/app/tools.ts", "export { tweak } from './tweak';"),
            ("/ws/app/barrel.ts", "export * from './tools';"),
            ("/ws/app/aliased.ts", "export { tweak } from '@app/tweak';"),
        ]);
        let value = eval(&loader);
        assert_eq!(
            read_at(&value, &["build", "outDir"]),
            &ConfigValue::Unknown,
            "{config}\n{tweak}: {value:?}"
        );
    }
    for config in [
        "import base from './base';\nimport { name } from './name';\nexport default { ...base, name: name() };",
        "import base from './base';\nimport { name } from './names';\nexport default { ...base, name: name() };",
    ] {
        let pure = files(&[
            ("/ws/app/config.ts", config),
            ("/ws/app/base.ts", base),
            ("/ws/app/name.ts", "export function name() { return 'app'; }"),
            (
                "/ws/app/names.ts",
                "export { name } from './name';\nexport * from 'some-package';",
            ),
        ]);
        let value = eval(&pure);
        assert_eq!(read_at(&value, &["build", "outDir"]), &s("dist"), "{config}: {value:?}");
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{config}: {value:?}");
    }
}

/// A barrel that re-exports a file holding no code (a declaration file, JSON, a stylesheet, an asset) changes no value
/// of a module: calling a function of it leaves the imported values known. One that re-exports a relative module the
/// loader neither follows nor knows to hold no code may change them.
#[test]
fn a_reexport_of_a_file_that_holds_no_code_changes_nothing() {
    for (reexport, out_dir) in [
        ("export * from './types';", s("dist")),
        ("export * from './types.js';", s("dist")),
        ("export { default as pkg } from './package.json';", s("dist")),
        ("export { default as styles } from './x.css';", s("dist")),
        ("export { default as logo } from './logo.svg';", s("dist")),
        ("export * from './missing';", ConfigValue::Unknown),
    ] {
        let tools = format!("{reexport}\nexport function name() {{ return 'app'; }}");
        let loader = files(&[
            (
                "/ws/app/config.ts",
                "import base from './base';\nimport { name } from './tools';\n\
                 export default { ...base, name: name() };",
            ),
            (
                "/ws/app/base.ts",
                "export default { root: 'src', build: { outDir: 'dist' } };",
            ),
            ("/ws/app/tools.ts", &*Box::leak(tools.into_boxed_str())),
            ("/ws/app/types.d.ts", "export interface Options { root: string }"),
            ("/ws/app/package.json", r#"{ "name": "app" }"#),
            ("/ws/app/x.css", ".a { color: red; }"),
            ("/ws/app/logo.svg", "<svg></svg>"),
        ]);
        let value = eval(&loader);
        assert_eq!(read_at(&value, &["build", "outDir"]), &out_dir, "{reexport}: {value:?}");
        if out_dir == s("dist") {
            assert_eq!(read_at(&value, &["root"]), &s("src"), "{reexport}: {value:?}");
            assert_eq!(read_at(&value, &["name"]), &s("app"), "{reexport}: {value:?}");
        }
    }
}

/// A call of a binding of a bare import the loader neither follows nor knows as installed (an alias of a project
/// directory) runs code of the project, which may change every value the config imports from a followed module; one
/// of an installed package does not.
#[test]
fn a_call_of_an_alias_the_loader_does_not_follow_changes_the_imported_values() {
    for (specifier, out_dir) in [("@app/setup", ConfigValue::Unknown), ("vite-setup", s("dist"))] {
        for (import, call) in [
            ("import { setup } from", "setup();"),
            ("import * as tools from", "tools.setup();"),
            ("import { Setup } from", "new Setup();"),
        ] {
            let config = format!("import base from './base';\n{import} '{specifier}';\n{call}\nexport default base;");
            let loader = files(&[
                ("/ws/app/config.ts", &*Box::leak(config.into_boxed_str())),
                (
                    "/ws/app/base.ts",
                    "export default { root: 'src', build: { outDir: 'dist' } };",
                ),
            ]);
            let value = eval(&loader);
            assert_eq!(
                read_at(&value, &["build", "outDir"]),
                &out_dir,
                "{specifier}: {call}: {value:?}"
            );
        }
    }
}

/// A module that hands out a function of a followed module that changes a value of a module, or of a bare import
/// neither followed nor installed (exporting it, as its default or inside an exported value), changes them when the
/// config calls that function through it; one that hands out a function changing nothing, and imports such a function
/// without using it, leaves them known.
#[test]
fn a_barrel_that_exports_an_imported_changing_function_changes_the_imported_values() {
    let tweaking = "import base from './base';\nexport function tweak() { base.build.outDir = 'out'; }";
    for (import, barrel, call) in [
        (
            "import { tweak } from './barrel';",
            "import { tweak } from './tweak';\nexport { tweak };",
            "tweak();",
        ),
        (
            "import tweak from './barrel';",
            "import { tweak } from './tweak';\nexport default tweak;",
            "tweak();",
        ),
        (
            "import { tools } from './barrel';",
            "import { tweak } from './tweak';\nexport const tools = { tweak };",
            "tools.tweak();",
        ),
        (
            "import { tweak } from './barrel';",
            "import { tweak } from '@app/tweak';\nexport { tweak };",
            "tweak();",
        ),
    ] {
        let config = format!("import base from './base';\n{import}\n{call}\nexport default base;");
        let loader = files(&[
            ("/ws/app/config.ts", &*Box::leak(config.into_boxed_str())),
            (
                "/ws/app/base.ts",
                "export default { root: 'src', build: { outDir: 'dist' } };",
            ),
            ("/ws/app/tweak.ts", tweaking),
            ("/ws/app/barrel.ts", barrel),
        ]);
        let value = eval(&loader);
        assert_eq!(
            read_at(&value, &["build", "outDir"]),
            &ConfigValue::Unknown,
            "{barrel}: {value:?}"
        );
    }
    let loader = files(&[
        (
            "/ws/app/config.ts",
            "import base from './base';\nimport { name } from './barrel';\nexport default { ...base, name: name() };",
        ),
        (
            "/ws/app/base.ts",
            "export default { root: 'src', build: { outDir: 'dist' } };",
        ),
        ("/ws/app/name.ts", "export function name() { return 'app'; }"),
        (
            "/ws/app/barrel.ts",
            "import { name } from './name';\nimport { tweak } from './tweak';\nexport { name };",
        ),
        ("/ws/app/tweak.ts", tweaking),
    ]);
    let value = eval(&loader);
    assert_eq!(read_at(&value, &["build", "outDir"]), &s("dist"), "{value:?}");
    assert_eq!(read_at(&value, &["name"]), &s("app"), "{value:?}");
}

/// A change of what a factory of a followed module returns, when the factory builds new objects, changes that part of
/// the result only: the rest of the config and the values imported from the module stay exact. One whose result holds
/// a value of its module at the place changed changes that value too.
#[test]
fn a_change_of_what_a_factory_that_builds_new_objects_returns_changes_that_part_only() {
    let factory = "import { defineConfig } from 'vitest/config';\n\
         export const shared = { environment: 'node' };\n\
         export const sharedFiles = ['./shared.ts'];\n\
         export function createVitestConfig(options = {}) {\n\
           return defineConfig({ test: { globals: true, setupFiles: [...(options.setupFiles ?? []), './base.ts'], \
         include: options.include ?? ['src/**/*.test.ts'] } });\n\
         }\n\
         export const createShared = () => ({ test: { setupFiles: sharedFiles, globals: true } });";
    let config = |create: &str| {
        let source = format!(
            "import {{ {create}, shared, sharedFiles }} from '@fx/base';\n\
             const config = {create}({{ setupFiles: ['./setup.ts'] }});\n\
             config.test.setupFiles.push('./extra.ts');\n\
             export default {{ ...config, shared, sharedFiles }};"
        );
        let loader = files(&[
            ("/ws/app/config.ts", &*Box::leak(source.into_boxed_str())),
            ("/ws/base/index.ts", factory),
        ]);
        eval(&loader)
    };
    let value = config("createVitestConfig");
    assert!(read_at(&value, &["test", "setupFiles"]).has_unknown(), "{value:?}");
    assert_eq!(
        read_at(&value, &["test", "globals"]),
        &ConfigValue::Bool(true),
        "{value:?}"
    );
    assert_eq!(
        read_at(&value, &["test", "include"]),
        &strings(&["src/**/*.test.ts"]),
        "{value:?}"
    );
    assert_eq!(
        read_at(&value, &["shared"]),
        &obj(vec![("environment", s("node"))]),
        "{value:?}"
    );
    assert_eq!(
        read_at(&value, &["sharedFiles"]),
        &strings(&["./shared.ts"]),
        "{value:?}"
    );
    let value = config("createShared");
    assert!(read_at(&value, &["test", "setupFiles"]).has_unknown(), "{value:?}");
    assert_eq!(
        read_at(&value, &["test", "globals"]),
        &ConfigValue::Bool(true),
        "{value:?}"
    );
    assert!(read_at(&value, &["sharedFiles"]).has_unknown(), "{value:?}");
}

/// A write through `this` changes no value of a module where `this` is an object a call makes anew: a constructor or a
/// field initialiser giving a key of the new instance a value, a method of a class no code run when the module is
/// loaded constructs, a method of an object literal a factory returns (a Vite or Rollup plugin's hooks). Constructing
/// or calling it leaves the imported values known.
#[test]
fn a_write_through_this_of_an_object_a_call_makes_leaves_the_imported_values_known() {
    for (shared, name, statement) in [
        (
            "export class EnvPlugin {\n  seen = [];\n  constructor(options) { this.options = options; this.count = 0; }\n  \
             apply(compiler) { this.compiler = compiler; this.seen.push(compiler); }\n}",
            "EnvPlugin",
            "const plugin = new EnvPlugin({ mode: 'test' });",
        ),
        (
            "export const envPlugin = () => ({ name: 'env', load(id) { this.addWatchFile(id); return null; } });",
            "envPlugin",
            "const plugin = envPlugin();",
        ),
        (
            "export function envPlugin() {\n  return {\n    name: 'env',\n    \
             buildStart() { this.emitFile({ type: 'asset', fileName: 'env.json' }); },\n    \
             transform: function (code) { this.warn(code); return code; },\n  };\n}",
            "envPlugin",
            "envPlugin();",
        ),
        (
            "export const make = () => ({ list: [], add(p) { const list = this.list; list.push(p); } });",
            "make",
            "make().add('x');",
        ),
    ] {
        let value = shared_config(shared, name, statement);
        assert_eq!(read_at(&value, &["plugins"]), &strings(&[]), "{shared}: {value:?}");
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{shared}: {value:?}");
    }
}

/// A module that imports `base` and whose exported `touch` runs `body`, where `make` makes an object holding
/// `base.plugins` at `list`, with `member` beside it.
fn made_by_touch(member: &str, body: &str) -> &'static str {
    let source = format!(
        "import {{ base }} from './base';\nconst make = () => ({{ list: base.plugins, {member} }});\n\
         export function touch() {{ {body} }}"
    );
    Box::leak(source.into_boxed_str())
}

const PUSHING_GETTER: &str = "get grow() { this.list.push('x'); return 0; }";
const PUSHING_SETTER: &str = "set grow(v) { this.list.push(v); }";
const PUSHING_ITERATOR: &str = "*[Symbol.iterator]() { this.list.push('x'); yield 0; }";

/// A getter, a setter or an iterator runs with its object as `this` where code reads or writes its key, destructures,
/// spreads or iterates the object, or gives it to a builtin (`JSON.stringify`), with no call of it written: a write
/// through `this` there changes what the object holds, as one in a method a call runs does, so calling the function
/// that runs it leaves the imported value the object holds unknown. A change of a binding taking what `this` holds is
/// such a write, a getter run by another one on its `this` writes through it too, and the object may hold the value
/// from a write after it is made. The explicit method calls are the controls.
#[test]
fn a_write_through_this_in_an_accessor_or_iterator_run_implicitly_changes_the_imported_values() {
    let class = "import { base } from './base';\nclass Box {\n  constructor() { this.list = base.plugins; }\n  \
                 get grow() { this.list.push('x'); return 0; }\n  add() { this.list.push('x'); }\n}\n";
    let shared = [
        made_by_touch(PUSHING_GETTER, "return make().grow;"),
        made_by_touch(
            "get grow() { const list = this.list; list.push('x'); return 0; }",
            "return make().grow;",
        ),
        made_by_touch(PUSHING_GETTER, "const { grow } = make(); return grow;"),
        made_by_touch(PUSHING_GETTER, "return { ...make() };"),
        made_by_touch(PUSHING_GETTER, "return JSON.stringify(make());"),
        made_by_touch(PUSHING_SETTER, "make().grow = 'x';"),
        made_by_touch(PUSHING_ITERATOR, "return [...make()];"),
        made_by_touch(PUSHING_ITERATOR, "for (const _ of make()) {}"),
        made_by_touch(PUSHING_ITERATOR, "const [first] = make(); return first;"),
        made_by_touch(
            &format!("{PUSHING_GETTER}, get twice() {{ return this.grow; }}"),
            "return make().twice;",
        ),
        &*Box::leak(format!("{class}export function touch() {{ return new Box().grow; }}").into_boxed_str()),
        "import { base } from './base';\n\
         const make = () => ({ list: base.plugins, *[Symbol.iterator]() { this.list.push('x'); yield 0; } });\n\
         function* all() { yield* make(); }\nexport function touch() { return all().next(); }",
        "import { base } from './base';\nconst make = () => ({ get grow() { this.list.push('x'); return 0; } });\n\
         export function touch() { const box = make(); box.list = base.plugins; return box.grow; }",
        made_by_touch("add() { this.list.push('x'); }", "make().add();"),
        &*Box::leak(format!("{class}export function touch() {{ new Box().add(); }}").into_boxed_str()),
    ];
    for shared in shared {
        let value = shared_config(shared, "touch", "touch();");
        assert_eq!(
            read_at(&value, &["plugins"]),
            &ConfigValue::Unknown,
            "{shared}: {value:?}"
        );
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{shared}: {value:?}");
    }
}

/// A getter or an iterator that only reads through `this`, also through a binding taking what it holds, or only gives
/// a key of an object a call makes and no code keeps a new value, changes nothing the config reads, nor does one no
/// code runs or a read of another key: the imported values stay known.
#[test]
fn an_accessor_or_iterator_that_changes_nothing_the_object_holds_leaves_the_imported_values_known() {
    for shared in [
        made_by_touch("get grow() { return this.list.length; }", "return make().grow;"),
        made_by_touch("get grow() { this.count = 1; return 0; }", "return make().grow;"),
        made_by_touch("*[Symbol.iterator]() { yield* this.list; }", "return [...make()];"),
        made_by_touch(
            "*[Symbol.iterator]() { for (const item of this.list) yield item; }",
            "return [...make()];",
        ),
        made_by_touch(PUSHING_GETTER, "make(); return 0;"),
        made_by_touch(PUSHING_GETTER, "return make().list.length;"),
    ] {
        let value = shared_config(shared, "touch", "touch();");
        assert_eq!(read_at(&value, &["plugins"]), &strings(&[]), "{shared}: {value:?}");
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{shared}: {value:?}");
    }
}

/// A config module that declares `box`, an object holding `member` and then `mode` and `list`, runs `statements` and
/// exports `box` with a `name` of its own. The member comes first, as the evaluator reads a key written before one it
/// cannot name (`[Symbol.iterator]`) as unknown, which that one may replace.
fn box_config(member: &str, statements: &str) -> ConfigValue {
    let member = if member.is_empty() {
        String::new()
    } else {
        format!("{member}, ")
    };
    let config = format!(
        "const box = {{ {member}mode: 'development', list: ['a'] }};\n{statements}\n\
         export default {{ box, name: 'app' }};"
    );
    eval(&files(&[("/ws/app/config.ts", &*Box::leak(config.into_boxed_str()))]))
}

/// What [`box_config`] leaves of `mode` and `list` of the box.
#[derive(Debug, Clone, Copy)]
enum Boxed {
    /// Both, after a change of all the box holds.
    Changed,
    /// `mode` alone, after a new value of it.
    Mode,
    /// `list` alone, which may hold more after a change of what it holds.
    List,
    /// Neither: both are as the module wrote them.
    Known,
}

/// Asserts what [`box_config`] of `member` and `statements` leaves of the box, as `expected` says.
fn assert_box(member: &str, statements: &str, expected: Boxed) {
    let value = box_config(member, statements);
    let written = (s("development"), strings(&["a"]));
    let (mode, list) = match expected {
        Boxed::Changed => (ConfigValue::Unknown, ConfigValue::Unknown),
        Boxed::Mode => (ConfigValue::Unknown, written.1),
        Boxed::List => (written.0, ConfigValue::Array(vec![s("a"), ConfigValue::Unknown])),
        Boxed::Known => written,
    };
    let case = format!("{member} / {statements}: {value:?}");
    assert_eq!(read_at(&value, &["box", "mode"]), &mode, "{case}");
    assert_eq!(read_at(&value, &["box", "list"]), &list, "{case}");
    assert_eq!(read_at(&value, &["name"]), &s("app"), "{case}");
}

/// A binding a getter gives `this` (`const self = this`), also one a function inside it reads, is `this` there, and
/// destructuring `this` reads its keys: a getter a read through it runs changes what the object holds, so a getter
/// running it so changes it too. A read through it of a key no getter has is the control.
#[test]
fn a_binding_taking_this_in_a_getter_runs_what_a_read_through_it_runs_on_the_object() {
    let through_self = format!("{PUSHING_GETTER}, get twice() {{ const self = this; return self.grow; }}");
    let in_function = format!(
        "{PUSHING_GETTER}, get twice() {{ const self = this; function run() {{ return self.grow; }} return run(); }}"
    );
    let destructured = format!("{PUSHING_GETTER}, get twice() {{ const {{ grow }} = this; return grow; }}");
    for member in [&through_self, &in_function, &destructured] {
        assert_box(member, "box.twice;", Boxed::Changed);
    }
    assert_box(
        &format!("{PUSHING_GETTER}, get twice() {{ const self = this; return self.mode; }}"),
        "box.twice;",
        Boxed::Known,
    );
    for member in [through_self, in_function, destructured] {
        let value = shared_config(made_by_touch(&member, "return make().twice;"), "touch", "touch();");
        assert_eq!(
            read_at(&value, &["plugins"]),
            &ConfigValue::Unknown,
            "{member}: {value:?}"
        );
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{member}: {value:?}");
    }
}

/// A parameter that destructures what it takes runs the getters and the iterator of the argument a call gives it: of
/// a function the module declares, its default, and a callback given the elements of a list. A parameter taking a
/// key no getter has is the control.
#[test]
fn a_destructured_parameter_runs_the_getters_and_iterators_of_what_it_takes() {
    for (member, statements, expected) in [
        (
            PUSHING_GETTER,
            "const take = ({ grow }) => grow;\nconst got = take(box);",
            Boxed::Changed,
        ),
        (
            PUSHING_GETTER,
            "const got = [box].map(({ grow }) => grow);",
            Boxed::Changed,
        ),
        (
            PUSHING_ITERATOR,
            "function first([item]) { return item; }\nconst got = first(box);",
            Boxed::Changed,
        ),
        (
            PUSHING_GETTER,
            "const take = ({ grow } = box) => grow;\nconst got = take();",
            Boxed::Changed,
        ),
        (
            PUSHING_GETTER,
            "const take = ({ mode }) => mode;\nconst got = take(box);",
            Boxed::Known,
        ),
        (
            PUSHING_GETTER,
            "const got = [box].map(({ mode }) => mode);",
            Boxed::Known,
        ),
    ] {
        assert_box(member, statements, expected);
    }
    for (member, body) in [
        (PUSHING_GETTER, "const take = ({ grow }) => grow; return take(make());"),
        (PUSHING_GETTER, "return [make()].map(({ grow }) => grow);"),
        (
            PUSHING_ITERATOR,
            "function first([item]) { return item; } return first(make());",
        ),
    ] {
        let value = shared_config(made_by_touch(member, body), "touch", "touch();");
        assert_eq!(
            read_at(&value, &["plugins"]),
            &ConfigValue::Unknown,
            "{body}: {value:?}"
        );
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{body}: {value:?}");
    }
}

/// A getter or a setter a descriptor defines (`Object.defineProperty`, `Object.defineProperties`,
/// `Reflect.defineProperty`, `Object.create`) runs where its key is read or written, with the object the call defines
/// it on as `this`. The object holds what `box.list` holds, so its change reaches it there; defining the getter alone
/// is the control.
#[test]
fn a_getter_or_setter_a_descriptor_defines_runs_with_the_defined_object_as_this() {
    let getter = "{ get() { this.list.push('x'); return 0; } }";
    let linked = "made.list = box.list;";
    let defined = [
        format!("const made = Object.defineProperty({{}}, 'grow', {getter});\n{linked}"),
        format!("const made = Object.defineProperties({{}}, {{ grow: {getter} }});\n{linked}"),
        format!("const made = Object.create(null, {{ grow: {getter} }});\n{linked}"),
    ];
    for definition in &defined {
        assert_box("", &format!("{definition}\nmade.grow;"), Boxed::List);
        assert_box("", definition, Boxed::Known);
    }
    let setter = format!(
        "const made = {{}};\nReflect.defineProperty(made, 'grow', {{ set(v) {{ this.list.push(v); }} }});\n{linked}"
    );
    assert_box("", &format!("{setter}\nmade.grow = 'x';"), Boxed::List);
    assert_box("", &setter, Boxed::Known);
    let shared = "import { base } from './base';\n\
                  const make = () => {\n  const made = { list: base.plugins };\n  \
                  Object.defineProperty(made, 'grow', { get() { this.list.push('x'); return 0; } });\n  \
                  return made;\n};\n\
                  export function touch() { return make().grow; }";
    let value = shared_config(shared, "touch", "touch();");
    assert_eq!(read_at(&value, &["plugins"]), &ConfigValue::Unknown, "{value:?}");
    assert_eq!(read_at(&value, &["name"]), &s("app"), "{value:?}");
}

/// A getter or a setter that gives a key of `this` a new value changes that key of the object it runs on, also one
/// code the module runs when it is loaded makes, or a call makes and keeps in a local: the key is unknown, and the
/// rest of the object stays known. A read of another key is the control.
#[test]
fn a_getter_giving_a_key_of_this_a_new_value_leaves_that_key_unknown() {
    let getter = "get prod() { this.mode = 'production'; return true; }";
    assert_box(getter, "box.prod;", Boxed::Mode);
    assert_box(
        "set prod(v) { this.mode = v; }",
        "box.prod = 'production';",
        Boxed::Mode,
    );
    assert_box(getter, "box.list;", Boxed::Known);
    for config in [
        "const config = { mode: 'development', get prod() { this.mode = 'production'; return true; } };\n\
         config.prod;\nexport default { mode: config.mode };",
        "export default () => {\n  \
         const config = { mode: 'development', get prod() { this.mode = 'production'; return true; } };\n  \
         config.prod;\n  return { mode: config.mode };\n};",
    ] {
        let value = eval(&files(&[("/ws/app/config.ts", config)]));
        assert_eq!(read_at(&value, &["mode"]), &ConfigValue::Unknown, "{config}: {value:?}");
    }
}

/// A getter run through an alias of a part of a local (`const { test } = config`) changes what that part holds in the
/// local itself, which the evaluator, seeing no call, does not change; the alias alone is the control.
#[test]
fn a_getter_run_through_an_alias_changes_the_local_it_takes_a_part_of() {
    for (run, list) in [("test.grow;", ConfigValue::Unknown), ("", strings(&["a"]))] {
        let config = format!(
            "export default () => {{\n  \
             const config = {{ test: {{ list: ['a'], get grow() {{ this.list.push('x'); return 0; }} }} }};\n  \
             const {{ test }} = config;\n  {run}\n  return config;\n}};"
        );
        let value = eval(&files(&[("/ws/app/config.ts", &*Box::leak(config.into_boxed_str()))]));
        assert_eq!(read_at(&value, &["test", "list"]), &list, "{run}: {value:?}");
    }
    let shared = "import { base } from './base';\nexport function touch() {\n  \
                  const config = { test: { list: base.plugins, get grow() { this.list.push('x'); return 0; } } };\n  \
                  const { test } = config;\n  return test.grow;\n}";
    let value = shared_config(shared, "touch", "touch();");
    assert_eq!(read_at(&value, &["plugins"]), &ConfigValue::Unknown, "{value:?}");
}

/// An iteration runs the `next` of the iterator `Symbol.iterator` returns, here the object itself; a method whose key
/// is computed but no `Symbol.iterator` is no iterator, and a read of a key is the control.
#[test]
fn an_iteration_runs_the_next_method_of_an_iterator_and_no_other_computed_method() {
    let iterator = "[Symbol.iterator]() { return this; }, next() { this.list.push('x'); return { done: true }; }";
    assert_box(iterator, "for (const item of box) {}", Boxed::Changed);
    assert_box(iterator, "box.mode;", Boxed::Known);
    assert_box(
        "[name]() { this.list.push('x'); }",
        "for (const item of box.list) {}",
        Boxed::Known,
    );
    let value = shared_config(
        made_by_touch(iterator, "for (const item of make()) {}"),
        "touch",
        "touch();",
    );
    assert_eq!(read_at(&value, &["plugins"]), &ConfigValue::Unknown, "{value:?}");
}

/// A tagged template gives its tag each value, which it may read the keys of or iterate; a value that holds no object
/// is the control.
#[test]
fn a_tagged_template_runs_the_getters_of_its_values() {
    assert_box(
        PUSHING_GETTER,
        "import { css } from 'lit';\ncss`${box}`;",
        Boxed::Changed,
    );
    assert_box(
        PUSHING_GETTER,
        "import { css } from 'lit';\ncss`${box.mode}`;",
        Boxed::Known,
    );
    let shared = "import { base } from './base';\nimport { css } from 'lit';\n\
                  const make = () => ({ list: base.plugins, get grow() { this.list.push('x'); return 0; } });\n\
                  export function touch() { return css`${make()}`; }";
    let value = shared_config(shared, "touch", "touch();");
    assert_eq!(read_at(&value, &["plugins"]), &ConfigValue::Unknown, "{value:?}");
}

/// A getter that calls a method of its object on `this` changes what the object holds only as that method does: a
/// method that only reads leaves it known, one that pushes to a list or gives a key a new value changes that, and a
/// method the object does not declare, or one code gives the key another function, may change anything.
#[test]
fn a_getter_calling_a_method_on_this_changes_what_the_method_changes() {
    let reading = "read() { return this.mode; }, get isProd() { return this.read() === 'p'; }";
    for (member, statements, expected) in [
        (reading, "box.isProd;", Boxed::Known),
        (
            "read() { this.list.push('x'); return this.mode; }, get isProd() { return this.read() === 'p'; }",
            "box.isProd;",
            Boxed::Changed,
        ),
        (
            "setProd() { this.mode = 'production'; }, get prod() { this.setProd(); return true; }",
            "box.prod;",
            Boxed::Mode,
        ),
        (
            "get isProd() { return this.lookup() === 'p'; }",
            "box.isProd;",
            Boxed::Changed,
        ),
        (
            reading,
            "box.read = function () { this.list.push('x'); return 'p'; };\nbox.isProd;",
            Boxed::Changed,
        ),
    ] {
        assert_box(member, statements, expected);
    }
    for (method, plugins) in [
        ("read() { return this.list.length; }", strings(&[])),
        ("read() { this.list.push('x'); return 0; }", ConfigValue::Unknown),
    ] {
        let member = format!("{method}, get isProd() {{ return this.read() > 0; }}");
        let value = shared_config(made_by_touch(&member, "return make().isProd;"), "touch", "touch();");
        assert_eq!(read_at(&value, &["plugins"]), &plugins, "{method}: {value:?}");
    }
}

/// A getter that changes what its object holds elsewhere in the module runs nowhere a known function that reads
/// no key is given a value (a `define…` helper, `path.join`), nor where a list is iterated or indexed, as no
/// iterator and no getter of an index is declared: the config's values stay known.
#[test]
fn a_getter_runs_nowhere_a_known_function_reads_no_key_nor_where_a_list_is_iterated() {
    let config = "import { defineConfig } from 'vite';\nimport path from 'node:path';\nconst plugins = ['a'];\n\
                  const helper = { list: [], get grow() { this.list.push('x'); return 0; } };\nhelper.grow;\n\
                  const root = path.join(plugins[0], 'src');\nconst unique = new Set(plugins);\n\
                  for (const plugin of plugins) {}\n\
                  export default defineConfig({ plugins: [...plugins], all: plugins, name: 'app' });";
    let value = eval(&files(&[("/ws/app/config.ts", config)]));
    assert_eq!(read_at(&value, &["all"]), &strings(&["a"]), "{value:?}");
    assert_eq!(read_at(&value, &["plugins"]), &strings(&["a"]), "{value:?}");
    assert_eq!(read_at(&value, &["name"]), &s("app"), "{value:?}");
}

/// A getter that runs another one on `this` that runs a third one so changes what the third one changes.
#[test]
fn a_chain_of_getters_run_on_this_changes_what_the_last_changes() {
    let chain = format!("{PUSHING_GETTER}, get twice() {{ return this.grow; }}, get thrice() {{ return this.twice; }}");
    assert_box(&chain, "box.thrice;", Boxed::Changed);
    assert_box(&chain, "box.mode;", Boxed::Known);
    let value = shared_config(made_by_touch(&chain, "return make().thrice;"), "touch", "touch();");
    assert_eq!(read_at(&value, &["plugins"]), &ConfigValue::Unknown, "{value:?}");
}

/// The `toString` a template literal runs and the `then` an `await` runs are not followed: what they change of the
/// object stays known.
#[test]
fn a_to_string_or_then_run_implicitly_is_not_followed() {
    assert_box(
        "toString() { this.list.push('x'); return ''; }",
        "const text = `${box}`;",
        Boxed::Known,
    );
    let then = "import { base } from './base';\n\
                const make = () => ({ list: base.plugins, then(resolve) { this.list.push('x'); resolve(0); } });\n\
                export async function touch() { await make(); }";
    for shared in [
        made_by_touch("toString() { this.list.push('x'); return ''; }", "return `${make()}`;"),
        then,
    ] {
        let value = shared_config(shared, "touch", "touch();");
        assert_eq!(read_at(&value, &["plugins"]), &strings(&[]), "{shared}: {value:?}");
    }
}

/// A write through `this` changes a value of a module where `this` may be one: in a function the module hands out,
/// which a namespace import calls on the module's namespace, and in a constructor through a key of the instance, which
/// may hold such a value.
#[test]
fn a_write_through_this_that_may_reach_a_value_of_a_module_changes_the_imported_values() {
    for (shared, config) in [
        (
            "export const plugins = [];\nexport function add(p) { this.plugins.push(p); }",
            "import * as shared from './shared';\nshared.add('x');\nexport default { plugins: shared.plugins };",
        ),
        (
            "import { base } from './base';\n\
             class Registry { constructor() { this.list = base.plugins; this.list.push('x'); } }\n\
             export const make = () => new Registry();",
            "import { base } from './base';\nimport { make } from './shared';\nmake();\n\
             export default { plugins: base.plugins };",
        ),
    ] {
        let loader = files(&[
            ("/ws/app/config.ts", config),
            ("/ws/app/shared.ts", shared),
            ("/ws/app/base.ts", "export const base = { plugins: [] };"),
        ]);
        let value = eval(&loader);
        assert!(read_at(&value, &["plugins"]).has_unknown(), "{shared}: {value:?}");
    }
}

/// A binding a function gives what `this` holds (`const list = this.plugins`, `const self = this`, `const { plugins } =
/// this`) is `this` there: a change through it, in a function bound to a value of a module, changes that value.
#[test]
fn a_write_through_a_binding_taking_this_changes_the_imported_values() {
    for body in [
        "const list = this.plugins; list.push(p);",
        "const self = this; self.plugins.push(p);",
        "const { plugins } = this; plugins.push(p);",
        "let list; list = this.plugins; list.push(p);",
        "this.plugins.forEach((list) => list.push(p));",
    ] {
        let shared = format!(
            "import {{ base }} from './base';\nfunction add(p) {{ {body} }}\nexport const addPlugin = add.bind(base);"
        );
        let value = shared_config(Box::leak(shared.into_boxed_str()), "addPlugin", "addPlugin('x');");
        assert_eq!(
            read_at(&value, &["plugins"]),
            &ConfigValue::Unknown,
            "{body}: {value:?}"
        );
        assert_eq!(read_at(&value, &["name"]), &s("app"), "{body}: {value:?}");
    }
}

/// `count` modules that each import every other one, read and write a shared object and call each other's functions:
/// a dense cycle of imports.
fn clique(count: usize) -> Files {
    let sources: Vec<(String, &'static str)> = (0..count)
        .map(|index| {
            let mut imports = String::new();
            let mut calls = String::new();
            for other in (0..count).filter(|other| *other != index) {
                writeln!(imports, "import {{ f{other}, s{other} }} from './m{other}';").unwrap();
                writeln!(calls, "  s{other}.seen = f{other}();").unwrap();
            }
            let source = format!(
                "{imports}export const s{index} = {{ name: 'm{index}' }};\nexport function f{index}() {{\n{calls}  return s{index};\n}}\n"
            );
            (format!("/ws/app/m{index}.ts"), &*Box::leak(source.into_boxed_str()))
        })
        .collect();
    let mut entries: Vec<(&str, &'static str)> =
        sources.iter().map(|(path, source)| (path.as_str(), *source)).collect();
    entries.push((
        "/ws/app/config.ts",
        "import { f0, s0 } from './m0';\nf0();\nexport default { plugins: [s0.name, f0().name] };",
    ));
    files(&entries)
}

/// A dense cycle of imports is analysed in bounded time: what one module's analysis finds under a pending module is
/// not kept, so without a bound on the analyses in all every re-entry repeats the cycle.
#[test]
fn a_dense_cycle_of_imports_is_analysed_in_bounded_time() {
    let loader = clique(12);
    let started = std::time::Instant::now();
    let config = eval(&loader);
    let elapsed = started.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(30), "took {elapsed:?}");
    assert!(config.get("plugins").is_some());
}

/// A module of `count` exported functions that all hand their parameters to one local function and an imported one,
/// as generated assertion modules do.
fn hub(count: usize) -> Files {
    let mut functions = String::new();
    for index in 0..count {
        writeln!(
            functions,
            "export function assert{index}(node, opts) {{\n  assert('T{index}', node, opts);\n}}"
        )
        .unwrap();
    }
    let source = format!(
        "import is from './is';\nfunction assert(type, node, opts) {{\n  if (!is(type, node, opts)) {{\n    throw new Error(type);\n  }}\n}}\n{functions}"
    );
    files(&[
        ("/ws/app/asserts.ts", Box::leak(source.into_boxed_str())),
        (
            "/ws/app/is.ts",
            "export default function is(type, node, opts) {\n  return node.type === type;\n}\n",
        ),
        (
            "/ws/app/config.ts",
            "import { assert0 } from './asserts';\nassert0({ type: 'T0' });\nexport default { plugins: ['a'] };",
        ),
    ])
}

/// A module whose many functions all reach one binding through their parameters is analysed in time linear in its
/// size.
#[test]
fn a_module_of_many_functions_sharing_one_helper_is_analysed_quickly() {
    let loader = hub(1200);
    let started = std::time::Instant::now();
    let config = eval(&loader);
    let elapsed = started.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(30), "took {elapsed:?}");
    assert_eq!(config.get("plugins"), Some(&ConfigValue::Array(vec![s("a")])));
}
