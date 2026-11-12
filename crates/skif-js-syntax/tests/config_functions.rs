//! Static evaluation of config functions: local bindings, local calls, `await`, `require.resolve`, glob calls.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ConfigValue, OWN_SPREAD, UNKNOWN_SPREAD, extract_config, extract_config_object};

fn extract(path: &str, source: &str) -> ConfigValue {
    extract_config_object(source, Path::new(path)).expect("config is understood")
}

fn s(v: &str) -> ConfigValue {
    ConfigValue::String(v.to_string())
}

fn strings(values: &[&str]) -> ConfigValue {
    ConfigValue::Array(values.iter().map(|v| s(v)).collect())
}

static UNKNOWN: ConfigValue = ConfigValue::Unknown;

fn get<'a>(value: &'a ConfigValue, path: &[&str]) -> &'a ConfigValue {
    path.iter()
        .fold(value, |current, key| current.get(key).unwrap_or(&UNKNOWN))
}

#[test]
fn local_const_in_returned_function() {
    let value = extract(
        "webpack.config.js",
        "module.exports = (env) => {\n  const loader = 'babel-loader';\n  const config = { entry: './src/index.js', \
         module: { rules: [{ loader }] } };\n  return config;\n};",
    );
    assert_eq!(get(&value, &["entry"]), &s("./src/index.js"));
    assert_eq!(
        get(&value, &["module", "rules"]),
        &ConfigValue::Array(vec![ConfigValue::Object(vec![(
            "loader".to_owned(),
            s("babel-loader"),
        )])])
    );
}

#[test]
fn async_function_with_await_and_promise_resolve() {
    let value = extract(
        "webpack.config.js",
        "const base = { mode: 'production' };\nmodule.exports = async () => {\n  const c = await Promise.resolve(base);\n  \
         return { ...c, extra: await Promise.resolve('x') };\n};",
    );
    assert_eq!(get(&value, &["mode"]), &s("production"));
    assert_eq!(get(&value, &["extra"]), &s("x"));
}

#[test]
fn export_default_function_declaration() {
    let value = extract(
        "vite.config.ts",
        "export default function () { const base = '/a/'; return { base }; }",
    );
    assert_eq!(get(&value, &["base"]), &s("/a/"));
}

#[test]
fn local_function_calls_bind_arguments() {
    let value = extract(
        "c.js",
        "function rule(test, loader) { return { test, loader }; }\nmodule.exports = () => {\n  \
         const helper = (name) => ({ use: name });\n  function twice(x) { return [x, x]; }\n  \
         return { a: rule('x', 'css-loader'), b: helper('y'), c: twice('z') };\n};",
    );
    assert_eq!(get(&value, &["a", "loader"]), &s("css-loader"));
    assert_eq!(get(&value, &["b", "use"]), &s("y"));
    assert_eq!(get(&value, &["c"]), &strings(&["z", "z"]));
}

#[test]
fn template_literal_with_local_part_keeps_the_static_text() {
    let value = extract(
        "c.js",
        "module.exports = () => {\n  const loaderPath = 'babel-loader';\n  return { loader: `${loaderPath}?cacheDirectory` };\n};",
    );
    assert_eq!(get(&value, &["loader"]), &s("babel-loader?cacheDirectory"));
}

#[test]
fn require_resolve_is_the_package() {
    let value = extract(
        "c.js",
        "module.exports = { loader: require.resolve('css-loader'), other: require.resolve(`style-loader`) };",
    );
    assert_eq!(get(&value, &["loader"]), &s("css-loader"));
    assert_eq!(get(&value, &["other"]), &s("style-loader"));
}

#[test]
fn nested_require_is_the_package_but_a_whole_config_is_not() {
    let value = extract(
        "postcss.config.js",
        "module.exports = { plugins: [require('autoprefixer'), require('tailwindcss')({ config: 1 }), require('./local')] };",
    );
    let plugins = get(&value, &["plugins"]).as_array().unwrap();
    assert_eq!(plugins[0], s("autoprefixer"));
    assert_eq!(plugins[1], s("tailwindcss"));
    assert_eq!(plugins[2], ConfigValue::Unknown);
    assert_eq!(
        extract("c.js", "module.exports = require('shared-config');"),
        ConfigValue::Unknown
    );
}

#[test]
fn storybook_absolute_path_idiom() {
    let source = "import { dirname, join } from 'path';\n\
        function getAbsolutePath(value) { return dirname(require.resolve(join(value, 'package.json'))); }\n\
        export default { framework: { name: getAbsolutePath('@storybook/react-vite') }, \
        addons: [getAbsolutePath('@storybook/addon-links')] };";
    let value = extract("main.ts", source);
    assert_eq!(get(&value, &["framework", "name"]), &s("@storybook/react-vite"));
    assert_eq!(get(&value, &["addons"]), &strings(&["@storybook/addon-links"]));
}

#[test]
fn storybook_idiom_with_the_path_namespace() {
    let source = "import path from 'node:path';\n\
        const pkg = (v) => path.dirname(require.resolve(path.join(v, 'package.json')));\n\
        export default { addons: [pkg('a-addon')] };";
    assert_eq!(get(&extract("main.js", source), &["addons"]), &strings(&["a-addon"]));
}

/// The directory of a package's resolved manifest is the package, in the ESM form the Storybook CLI writes too; the
/// directory of a relative manifest stays a path.
#[test]
fn resolved_package_manifest_directories_are_the_package() {
    let cases = [
        (
            "import { dirname } from 'path';\nimport { fileURLToPath } from 'url';\n\
             function getAbsolutePath(value: string): string {\n\
               return dirname(fileURLToPath(import.meta.resolve(`${value}/package.json`)));\n}\n\
             export default { addons: [getAbsolutePath('@storybook/addon-a11y')] };",
            "@storybook/addon-a11y",
        ),
        (
            "import path from 'node:path';\nimport url from 'node:url';\n\
             const pkg = (v) => path.dirname(url.fileURLToPath(import.meta.resolve(path.join(v, 'package.json'))));\n\
             export default { addons: [pkg('a-addon')] };",
            "a-addon",
        ),
        (
            "import { dirname } from 'path';\n\
             export default { addons: [dirname(require.resolve('b-addon/package.json'))] };",
            "b-addon",
        ),
        (
            "import { dirname } from 'path';\n\
             export default { addons: [dirname(require.resolve('./local/package.json'))] };",
            "./local",
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(
            get(&extract("main.ts", source), &["addons"]),
            &strings(&[expected]),
            "{source}"
        );
    }
}

/// A manifest directory names a package only for a bare specifier, a scoped one written in several `join` arguments
/// included; a relative manifest stays a path, and an absolute one, a subpath import or a path through `..` is
/// unknown rather than a package or a path.
#[test]
fn only_a_bare_manifest_directory_is_a_package() {
    let value = extract(
        "main.ts",
        "import { dirname, join } from 'path';\n\
         export default {\n\
           scopedString: dirname(require.resolve('@a/b/package.json')),\n\
           scopedJoin: dirname(require.resolve(join('@s', 'n', 'package.json'))),\n\
           relativeJoin: dirname(require.resolve(join('./local', 'package.json'))),\n\
           absoluteJoin: dirname(require.resolve(join('/abs/x', 'package.json'))),\n\
           absoluteString: dirname(require.resolve('/abs/x/package.json')),\n\
           subpathImport: dirname(require.resolve('#internal/package.json')),\n\
           parent: dirname(require.resolve('a/../b/package.json')),\n\
           computedJoin: dirname(require.resolve(join(name(), 'package.json'))),\n\
           unknownSpecifier: import.meta.resolve(specifier),\n\
         };",
    );
    let pairs = [
        ("scopedString", s("@a/b")),
        ("scopedJoin", s("@s/n")),
        ("relativeJoin", s("./local")),
        ("absoluteJoin", ConfigValue::Unknown),
        ("absoluteString", ConfigValue::Unknown),
        ("subpathImport", ConfigValue::Unknown),
        ("parent", ConfigValue::Unknown),
        ("computedJoin", ConfigValue::Unknown),
        ("unknownSpecifier", ConfigValue::Unknown),
    ];
    for (key, expected) in pairs {
        assert_eq!(get(&value, &[key]), &expected, "{key}");
    }
}

#[test]
fn import_meta_resolve_is_the_specifier() {
    let value = extract(
        "c.mjs",
        "export default { loader: import.meta.resolve('css-loader'), local: import.meta.resolve('./loader.js') };",
    );
    assert_eq!(get(&value, &["loader"]), &s("css-loader"));
    assert_eq!(get(&value, &["local"]), &s("./loader.js"));
}

#[test]
fn glob_calls_become_their_patterns() {
    let cases = [
        ("import { glob } from 'glob';", "await glob('src/*/index.{ts,tsx}')"),
        ("import fg from 'fast-glob';", "fg.sync('src/*/index.{ts,tsx}')"),
        (
            "import { globSync } from 'tinyglobby';",
            "globSync('src/*/index.{ts,tsx}')",
        ),
        (
            "const globby = require('globby');",
            "await globby(['src/*/index.{ts,tsx}'])",
        ),
    ];
    for (import, call) in cases {
        let source = format!("{import}\nexport default async () => ({{ entry: {call} }});");
        let value = extract("rollup.config.mjs", &source);
        assert_eq!(get(&value, &["entry"]), &strings(&["src/*/index.{ts,tsx}"]), "{call}");
    }
}

#[test]
fn only_functions_imported_from_glob_packages_are_glob_calls() {
    let value = extract("c.js", "const glob = (p) => p;\nexport default { entry: glob('a') };");
    assert_eq!(get(&value, &["entry"]), &s("a"));
    let value = extract("c.js", "export default { entry: glob('a') };");
    assert_eq!(get(&value, &["entry"]), &ConfigValue::Unknown);
}

#[test]
fn member_assignments_to_a_local_object() {
    let value = extract(
        "c.js",
        "module.exports = () => {\n  const config = { a: 1 };\n  config.b = 'x';\n  config.a = 2;\n  \
         config.plugins.push(1);\n  return config;\n};",
    );
    assert_eq!(get(&value, &["a"]), &ConfigValue::Number(2.0));
    assert_eq!(get(&value, &["b"]), &s("x"));
    assert!(
        value.has_unknown(),
        "an unfollowed mutation leaves the object partly unknown"
    );
}

#[test]
fn local_bindings_shadow_top_level_constants() {
    let value = extract(
        "c.js",
        "const name = 'outer';\nmodule.exports = (name) => {\n  const pick = (name) => name;\n  return { a: name, b: pick('inner') };\n};",
    );
    assert_eq!(get(&value, &["a"]), &ConfigValue::Unknown);
    assert_eq!(get(&value, &["b"]), &s("inner"));
}

#[test]
fn recursion_and_call_blowup_stay_bounded() {
    let value = extract(
        "c.js",
        "function f(x) { return [f(x), f(x)]; }\nexport default { a: f(1), b: 'ok' };",
    );
    assert_eq!(get(&value, &["b"]), &s("ok"));
    let value = extract(
        "c.js",
        "function loop() { return loop(); }\nexport default { a: loop() };",
    );
    assert_eq!(get(&value, &["a"]), &ConfigValue::Unknown);
}

#[test]
fn karma_function_form_evaluates_the_object_given_to_set() {
    let value = extract(
        "karma.conf.js",
        "module.exports = function (config) {\n  const base = '/src';\n  config.set({\n    basePath: base,\n    frameworks: ['jasmine'],\n    \
         plugins: [require('karma-jasmine')],\n  });\n};",
    );
    assert_eq!(get(&value, &["basePath"]), &s("/src"));
    assert_eq!(get(&value, &["frameworks"]), &strings(&["jasmine"]));
}

#[test]
fn karma_set_with_a_local_object_and_an_arrow_function() {
    let value = extract(
        "karma.conf.js",
        "module.exports = (karma) => {\n  const configuration = { frameworks: ['mocha'] };\n  configuration.browsers = ['Chrome'];\n  \
         karma.set(configuration);\n};",
    );
    assert_eq!(get(&value, &["frameworks"]), &strings(&["mocha"]));
    assert_eq!(get(&value, &["browsers"]), &strings(&["Chrome"]));
}

#[test]
fn a_set_call_on_something_else_is_not_the_config() {
    let value = extract(
        "karma.conf.js",
        "module.exports = function (config) {\n  cache.set({ frameworks: ['nope'] });\n  return { frameworks: ['yes'] };\n};",
    );
    assert_eq!(get(&value, &["frameworks"]), &strings(&["yes"]));
}

#[test]
fn karma_default_export_function_with_a_set_call() {
    let value = extract(
        "karma.conf.ts",
        "export default function (config: any) { config.set({ basePath: '..' }); }",
    );
    assert_eq!(get(&value, &["basePath"]), &s(".."));
}

#[test]
fn merge_config_merges_the_known_objects_and_marks_the_imported_one() {
    let value = extract(
        "vitest.config.ts",
        "import { mergeConfig, defineConfig } from 'vitest/config';\n\
         import viteConfig from './vite.config';\n\
         export default mergeConfig(viteConfig, defineConfig({ test: { setupFiles: ['./config/setup.ts'], \
         include: ['checks/**/*.ts'] } }));",
    );
    assert_eq!(get(&value, &["test", "setupFiles"]), &strings(&["./config/setup.ts"]));
    assert_eq!(get(&value, &["test", "include"]), &strings(&["checks/**/*.ts"]));
    assert_eq!(value.get("..."), Some(&ConfigValue::Unknown));
}

#[test]
fn merge_config_concatenates_arrays_and_merges_nested_objects() {
    let value = extract(
        "vite.config.ts",
        "const base = { test: { include: ['a/**'], poolOptions: { threads: 1 } } };\n\
         export default mergeConfig(base, { test: { include: ['b/**'], poolOptions: { forks: 2 } } }, true);",
    );
    assert_eq!(get(&value, &["test", "include"]), &strings(&["a/**", "b/**"]));
    assert_eq!(
        get(&value, &["test", "poolOptions", "threads"]),
        &ConfigValue::Number(1.0)
    );
    assert_eq!(
        get(&value, &["test", "poolOptions", "forks"]),
        &ConfigValue::Number(2.0)
    );
    assert_eq!(value.get("..."), None);
}

#[test]
fn object_assign_is_shallow_and_defu_prefers_the_first_argument() {
    let assign = extract(
        "a.config.js",
        "module.exports = Object.assign({}, { files: ['a'] }, { files: ['b'] });",
    );
    assert_eq!(get(&assign, &["files"]), &strings(&["b"]));
    let defu = extract(
        "b.config.js",
        "import { defu } from 'defu';\nexport default defu({ mode: 'x' }, { mode: 'y', extra: 1 });",
    );
    assert_eq!(get(&defu, &["mode"]), &s("x"));
    assert_eq!(get(&defu, &["extra"]), &ConfigValue::Number(1.0));
}

#[test]
fn merge_is_a_combinator_only_when_it_is_not_another_librarys_merge() {
    let rxjs = extract(
        "d.config.js",
        "import { merge } from 'rxjs';\nexport default { a: merge({ x: 1 }, { y: 2 }) };",
    );
    assert_eq!(get(&rxjs, &["a"]), &UNKNOWN);
    let webpack = extract(
        "e.config.js",
        "import { merge } from 'webpack-merge';\nexport default merge({ x: 1 }, { y: 2 });",
    );
    assert_eq!(get(&webpack, &["x"]), &ConfigValue::Number(1.0));
    assert_eq!(get(&webpack, &["y"]), &ConfigValue::Number(2.0));
    let member = extract("f.config.js", "export default _.merge({ x: 1 }, { y: 2 });");
    assert_eq!(get(&member, &["y"]), &ConfigValue::Number(2.0));
    let stream = extract("g.config.js", "export default { a: stream.merge({ x: 1 }) };");
    assert_eq!(get(&stream, &["a"]), &UNKNOWN);
}

#[test]
fn a_merge_of_unknown_operands_leaves_only_the_marker() {
    let value = extract("c.config.js", "export default { a: merge(x, y) };");
    assert_eq!(
        get(&value, &["a"]),
        &ConfigValue::Object(vec![("...".to_owned(), ConfigValue::Unknown)])
    );
}

#[test]
fn a_conditional_with_a_known_test_takes_its_branch() {
    let value = extract(
        "vite.config.ts",
        "const pins = [];\nconst on = 'yes';\n\
         export default { ...(pins.length ? { resolve: {} } : {}), a: on ? 'x' : 'y', b: !pins.length ? 1 : 2, \
         c: 'ab'.length, d: [1, ...unknown].length };",
    );
    assert_eq!(
        value,
        ConfigValue::Object(vec![
            ("a".to_owned(), s("x")),
            ("b".to_owned(), ConfigValue::Number(1.0)),
            ("c".to_owned(), ConfigValue::Number(2.0)),
            ("d".to_owned(), ConfigValue::Unknown),
        ])
    );
}

#[test]
fn logical_operators_on_known_values() {
    let value = extract(
        "vite.config.ts",
        "const none = null;\nconst list = ['a'];\n\
         export default { a: none ?? 'd', b: list ?? 'd', c: '' || 'e', d: list && 'f', e: 0 && 'g', \
         f: env() ?? 'h', g: env() || 'i' };",
    );
    let pairs = [
        ("a", s("d")),
        ("b", strings(&["a"])),
        ("c", s("e")),
        ("d", s("f")),
        ("e", ConfigValue::Number(0.0)),
        ("f", ConfigValue::Unknown),
        ("g", ConfigValue::Unknown),
    ];
    for (key, expected) in pairs {
        assert_eq!(get(&value, &[key]), &expected, "{key}");
    }
}

#[test]
fn destructured_parameters_take_their_defaults() {
    let value = extract(
        "vite.config.ts",
        "const pinCjs = ['outer'];\n\
         const create = (options = {}, { pinCjs = [], mode = 'dev', ...rest } = {}) => ({ \
         test: options, mode, rest, ...(pinCjs.length ? { resolve: { alias: pinCjs } } : {}) });\n\
         export default [create(), create({ a: 1 }, { pinCjs: ['zod'] }), create(undefined, unknown())];",
    );
    let configs = value.as_array().unwrap();
    assert_eq!(
        configs[0],
        ConfigValue::Object(vec![
            ("test".to_owned(), ConfigValue::Object(vec![])),
            ("mode".to_owned(), s("dev")),
            ("rest".to_owned(), ConfigValue::Unknown),
        ])
    );
    assert_eq!(get(&configs[1], &["resolve", "alias"]), &strings(&["zod"]));
    assert_eq!(get(&configs[1], &["mode"]), &s("dev"));
    // An unknown argument leaves its names unknown, not bound to the outer `pinCjs`.
    assert_eq!(get(&configs[2], &["mode"]), &ConfigValue::Unknown);
    assert_eq!(
        configs[2].get("resolve"),
        Some(&ConfigValue::Object(vec![("alias".to_owned(), ConfigValue::Unknown)]))
    );
}

#[test]
fn a_conditional_with_an_unknown_test_keeps_what_both_branches_share() {
    let value = extract(
        "vitest.config.ts",
        "export default { build: { outDir: 'dist' }, \
         ...(process.env.CI ? { test: { include: ['a'], environment: 'node' } } : { test: { environment: 'node' } }), \
         ...(process.env.X ? { a: 1 } : unknown()) };",
    );
    assert_eq!(get(&value, &["build", "outDir"]), &s("dist"));
    assert_eq!(get(&value, &["test", "environment"]), &s("node"));
    // A list only one spread branch sets is credited whole: the other branch adds nothing to it.
    assert_eq!(get(&value, &["test", "include"]), &strings(&["a"]));
    // A branch that is not an object leaves the spread unknown.
    assert!(value.get("...").is_some());
}

#[test]
fn a_spread_guarded_by_an_unknown_test_adds_the_keys_of_its_object() {
    let value = extract(
        "vitest.config.ts",
        "const tmp = process.env.TEST_TMPDIR;\n\
         export default { ...(tmp && { cacheDir: `${tmp}/vite` }), ...((process.env.A && { base: '/' })), \
         test: { include: ['**/*.test.ts'] } };",
    );
    assert!(value.get("...").is_none(), "{value:?}");
    // A key the falsy branch leaves out may be present or not.
    assert_eq!(value.get("cacheDir"), Some(&ConfigValue::Unknown));
    assert_eq!(value.get("base"), Some(&ConfigValue::Unknown));
    assert_eq!(get(&value, &["test", "include"]), &strings(&["**/*.test.ts"]));
}

#[test]
fn a_conditional_spread_credits_the_lists_of_every_branch() {
    let value = extract(
        "vitest.config.ts",
        "export default {\n\
           test: { include: ['src/**/*.test.ts'], environment: 'node' },\n\
           ...(process.env.A ? { test: { include: ['a/**'], setupFiles: ['./setup-a.ts'], environment: 'node' } } \
             : { test: { include: ['b/**'], environment: 'node' } }),\n\
           ...(process.env.B && { build: { rollupOptions: { input: ['b.html'] } } }),\n\
         };",
    );
    assert!(value.get("...").is_none(), "{value:?}");
    assert_eq!(get(&value, &["test", "include"]), &strings(&["a/**", "b/**"]));
    assert_eq!(get(&value, &["test", "setupFiles"]), &strings(&["./setup-a.ts"]));
    assert_eq!(get(&value, &["test", "environment"]), &s("node"));
    assert_eq!(get(&value, &["build", "rollupOptions", "input"]), &strings(&["b.html"]));
}

#[test]
fn a_conditional_spread_credits_a_string_and_a_list_alike() {
    let value = extract(
        "vitest.config.ts",
        "export default { ...(process.env.A ? { test: { setupFiles: './a.ts' } } \
         : { test: { setupFiles: ['./b.ts'] } }) };",
    );
    // A string where another branch has a list is a list of one.
    assert!(value.get("...").is_none(), "{value:?}");
    assert_eq!(get(&value, &["test", "setupFiles"]), &strings(&["./a.ts", "./b.ts"]));
}

#[test]
fn a_guarded_spread_keeps_what_its_object_does_not_replace() {
    let value = extract(
        "vitest.config.ts",
        "export default { test: { include: ['x/**'] }, root: 'web', \
         ...(process.env.A && { test: { setupFiles: ['./s.ts'] } }), ...(process.env.B ? { root: 'app' } : {}) };",
    );
    // The spread replaces `test` as a whole in one branch, so each branch's lists are credited.
    assert_eq!(get(&value, &["test", "include"]), &strings(&["x/**"]));
    assert_eq!(get(&value, &["test", "setupFiles"]), &strings(&["./s.ts"]));
    // `root` is `web` or `app`.
    assert_eq!(get(&value, &["root"]), &ConfigValue::OneOf(vec![s("app"), s("web")]));
}

#[test]
fn a_spread_branch_that_is_not_known_leaves_the_marker() {
    let cases = [
        "export default { ...(process.env.BASE || { a: 1 }) };",
        "export default { ...(process.env.BASE ?? { a: 1 }) };",
        "export default { ...(process.env.A ? base() : {}) };",
        "export default { ...(process.env.A && base()) };",
        "export default { ...(process.env.A ? { ...base(), test: { include: ['a'] } } : {}) };",
    ];
    for source in cases {
        let value = extract("vitest.config.ts", source);
        assert!(value.get("...").is_some(), "{source}: {value:?}");
    }
    let value = extract(
        "vitest.config.ts",
        "export default { ...(process.env.A ? { ...base(), test: { include: ['a'] } } : {}) };",
    );
    assert_eq!(get(&value, &["test", "include"]), &strings(&["a"]));
}

#[test]
fn a_spread_of_a_boolean_or_null_adds_nothing() {
    let value = extract(
        "vitest.config.ts",
        "export default { ...(process.env.MODE === 'ci' || { a: [1] }), ...(!process.env.B ?? { b: [2] }), \
         ...null, ...false };",
    );
    // A comparison is `true` or `false`, so `||` adds its object or nothing; a negation is never nullish, so `??`
    // never reaches its object.
    assert_eq!(
        value,
        ConfigValue::Object(vec![(
            "a".to_owned(),
            ConfigValue::Array(vec![ConfigValue::Number(1.0)])
        )])
    );
}

#[test]
fn a_spread_branch_of_an_empty_string_adds_nothing() {
    let value = extract(
        "vitest.config.ts",
        "export default { ...(process.env.A ? '' : { test: { setupFiles: ['./s.ts'] } }) };",
    );
    assert!(value.get("...").is_none(), "{value:?}");
    assert_eq!(get(&value, &["test", "setupFiles"]), &strings(&["./s.ts"]));
}

#[test]
fn a_void_left_of_a_coalesce_reaches_its_object() {
    let value = extract(
        "vitest.config.ts",
        "export default { ...(void 0 ?? { test: { setupFiles: ['./s.ts'] } }) };",
    );
    // `void x` is `undefined`, so `??` takes its object.
    assert!(value.get("...").is_none(), "{value:?}");
    assert_eq!(get(&value, &["test", "setupFiles"]), &strings(&["./s.ts"]));
}

#[test]
fn an_early_return_is_not_skipped_for_the_last_one() {
    let value = extract(
        "vitest.config.ts",
        "const make = (mode) => {\n\
           if (process.env.CI) return { test: { include: ['ci'] }, root: '.' };\n\
           return { test: { include: ['all'] }, root: '.' };\n\
         };\n\
         const known = (mode = 'dev') => { if (mode) { return { a: 1 }; } return { a: 2 }; };\n\
         const loop = () => { for (const x of y) { return { a: 1 }; } return { a: 2 }; };\n\
         const big = () => { if (env()) { const a = 1; return { a }; } return { a: 2 }; };\n\
         const none = () => { if (env()) { setup(); } return { a: 3 }; };\n\
         export default [make(), known(), loop(), big(), none()];",
    );
    let configs = value.as_array().unwrap();
    // The two returns share `root`; `test.include` differs, so it is one of the two rather than the last return's.
    assert_eq!(get(&configs[0], &["root"]), &s("."));
    assert_eq!(
        get(&configs[0], &["test", "include"]),
        &ConfigValue::OneOf(vec![strings(&["ci"]), strings(&["all"])])
    );
    // A known test takes its branch.
    assert_eq!(get(&configs[1], &["a"]), &ConfigValue::Number(1.0));
    assert_eq!(configs[2], ConfigValue::Unknown);
    assert_eq!(configs[3], ConfigValue::Unknown);
    assert_eq!(
        configs[4],
        ConfigValue::Object(vec![("a".to_owned(), ConfigValue::Number(3.0))])
    );
}

#[test]
fn a_declaration_in_a_block_does_not_replace_the_outer_binding() {
    let value = extract(
        "vite.config.ts",
        "const outer = () => { const c = { a: 1 }; if (true) { const c = { a: 2 }; } { let c = 3; } return c; };\n\
         const fn = () => { const f = () => 1; if (1) { const f = () => 2; } return { r: f() }; };\n\
         const hoisted = () => { if (true) { var v = 1; } return { v }; };\n\
         export default [outer(), fn(), hoisted()];",
    );
    let configs = value.as_array().unwrap();
    assert_eq!(get(&configs[0], &["a"]), &ConfigValue::Number(1.0));
    assert_eq!(get(&configs[1], &["r"]), &ConfigValue::Number(1.0));
    assert_eq!(get(&configs[2], &["v"]), &ConfigValue::Unknown);
}

#[test]
fn what_code_may_or_may_not_write_is_unknown_after_it() {
    let value = extract(
        "vite.config.ts",
        "const plugins = [];\nif (process.env.A) plugins.push('x');\n\
         const base = { a: 1 };\nfor (const k of keys) base[k] = 1;\n\
         const make = () => {\n\
           let mode = 'a'; const list = []; const c = { a: 1 }; let n = 0;\n\
           if (env()) { mode = 'b'; }\n\
           for (const x of y) list.push(x);\n\
           env() && (c.a = 2);\n\
           if (env()) n++;\n\
           return { mode, some: list.length ? 1 : 2, c, n: n ? 1 : 2 };\n\
         };\n\
         export default [{ some: plugins.length ? 1 : 2, base }, make()];",
    );
    let configs = value.as_array().unwrap();
    let one_or_two = ConfigValue::OneOf(vec![ConfigValue::Number(1.0), ConfigValue::Number(2.0)]);
    assert_eq!(get(&configs[0], &["some"]), &one_or_two);
    assert!(get(&configs[0], &["base"]).get("...").is_some());
    assert_eq!(get(&configs[1], &["mode"]), &ConfigValue::OneOf(vec![s("a"), s("b")]));
    for key in ["some", "n"] {
        assert_eq!(get(&configs[1], &[key]), &one_or_two, "{key}");
    }
    assert!(get(&configs[1], &["c"]).get("...").is_some());
}

#[test]
fn a_returned_key_does_not_depend_on_what_else_the_object_reads() {
    let body = "let mode = 'a'; const list = []; const c = { a: 1 }; let n = 0;\n\
                if (env()) { mode = 'b'; }\n\
                for (const x of y) list.push(x);\n\
                env() && (c.a = 2);\n\
                if (env()) n++;\n";
    let one_or_two = ConfigValue::OneOf(vec![ConfigValue::Number(1.0), ConfigValue::Number(2.0)]);
    // The returned object, the keys that are one of 1 and 2, and the keys whose object may hold more keys.
    let cases: [(&str, &[&str], &[&str]); 4] = [
        ("{ mode }", &[], &[]),
        ("{ mode, n: n ? 1 : 2 }", &["n"], &[]),
        ("{ mode, c }", &[], &["c"]),
        ("{ mode, some: list.length ? 1 : 2 }", &["some"], &[]),
    ];
    for (returned, one_of_two, opened) in cases {
        let source =
            format!("const make = () => {{\n{body}return {returned};\n}};\nexport default [{{ base: 1 }}, make()];");
        let value = extract("vite.config.ts", &source);
        let config = &value.as_array().unwrap()[1];
        assert_eq!(
            get(config, &["mode"]),
            &ConfigValue::OneOf(vec![s("a"), s("b")]),
            "{returned}"
        );
        for key in one_of_two {
            assert_eq!(get(config, &[*key]), &one_or_two, "{returned}: {key}");
        }
        for key in opened {
            assert!(open(get(config, &[*key])), "{returned}: {key}");
        }
    }
}

#[test]
fn a_top_level_binding_is_not_followed_through_the_statements_that_reassign_it() {
    // The evaluator reads top-level constants by name, not the module's statements in order, so a top-level `let` is
    // unknown whatever assigns it.
    let value = extract(
        "vite.config.ts",
        "let mode = 'a';\nif (env()) { mode = 'b'; }\nexport default { mode };",
    );
    assert_eq!(get(&value, &["mode"]), &ConfigValue::Unknown);
}

#[test]
fn a_return_in_an_else_branch_is_not_read_after_the_code_that_follows() {
    let value = extract(
        "vite.config.ts",
        "const make = () => {\n\
           const c = { a: 1 };\n\
           if (env()) { setup(); } else { return c; }\n\
           c.a = 2;\n\
           return c;\n\
         };\n\
         const pick = ({ mode = 'dev' }) => ({ mode });\n\
         export default [make(), pick({ mode: 'prod', ...options() }), pick({ ...options(), mode: 'prod' })];",
    );
    let configs = value.as_array().unwrap();
    // One path returns `{ a: 1 }` and the other `{ a: 2 }`.
    assert_eq!(
        get(&configs[0], &["a"]),
        &ConfigValue::OneOf(vec![ConfigValue::Number(2.0), ConfigValue::Number(1.0)])
    );
    // A spread after a key may replace it.
    assert_eq!(get(&configs[1], &["mode"]), &ConfigValue::Unknown);
    assert_eq!(get(&configs[2], &["mode"]), &s("prod"));
}

/// Whether `value` is an object that says it may hold more than the keys it lists.
fn open(value: &ConfigValue) -> bool {
    value.get("...").is_some()
}

#[test]
fn a_change_through_an_alias_a_call_or_a_closure_reaches_the_original() {
    let value = extract(
        "vite.config.ts",
        "const alias = () => { const cfg = { a: 1 }; const a = cfg; if (env) a.x = 1; return cfg; };\n\
         const nested = () => { const t = { include: ['a'] }; const cfg = { test: t }; t.include.push('b'); return cfg; };\n\
         const closure = () => { const cfg = { a: 1 }; const f = () => { cfg.a = 2; }; f(); return cfg; };\n\
         const callback = () => { const paths = {}; dirs.forEach((d) => { paths[d] = d; }); let m = 'a'; [1].forEach(() => { m = 'b'; }); return { paths, m }; };\n\
         const helper = () => { const cfg = { a: 1 }; configure(cfg); return cfg; };\n\
         const local = () => { const cfg = { a: 1 }; const set = (c) => { c.a = 2; return c; }; const r = set(cfg); return cfg; };\n\
         const top = { a: 1 }; const view = top; view.a = 2;\n\
         const captured = { a: 1 }; function change() { captured.a = 2; } change();\n\
         export default [alias(), nested(), closure(), callback(), helper(), local(), top, captured];",
    );
    let configs = value.as_array().unwrap();
    for (index, config) in configs.iter().enumerate() {
        if index != 3 {
            assert!(open(config), "{index}: {config:?}");
        }
    }
    assert!(open(get(&configs[3], &["paths"])));
    assert_eq!(get(&configs[3], &["m"]), &ConfigValue::Unknown);
}

/// A `resolve` call (`require.resolve`, the function `createRequire` returns) a callback makes does not change the
/// object it is called on, so options merged next to the result of the function that makes it stay known; a method that
/// may change it still leaves them partly unknown.
#[test]
fn a_resolve_call_in_a_callback_changes_nothing_it_shares() {
    let value = extract(
        "vitest.config.ts",
        "import { createRequire } from 'node:module';\n\
         const pin = (deps) => { const req = createRequire(deps); return deps.map((dep) => req.resolve(dep)); };\n\
         const hook = (deps) => { const req = createRequire(deps); return deps.map((dep) => req.register(dep)); };\n\
         const make = (options = {}) => ({ test: { globals: true, ...options }, alias: pin([]) });\n\
         const hooked = (options = {}) => ({ test: { globals: true, ...options }, alias: hook([]) });\n\
         export default [make({ setupFiles: ['./setup.ts'] }), hooked({ setupFiles: ['./setup.ts'] })];",
    );
    let configs = value.as_array().unwrap();
    assert_eq!(
        get(&configs[0], &["test"]),
        &ConfigValue::Object(vec![
            ("globals".to_owned(), ConfigValue::Bool(true)),
            ("setupFiles".to_owned(), strings(&["./setup.ts"])),
        ])
    );
    assert!(open(get(&configs[1], &["test"])), "{:?}", configs[1]);
}

#[test]
fn a_write_inside_an_expression_or_a_delete_is_not_lost() {
    let value = extract(
        "vite.config.ts",
        "const list = ['a']; const n = list.push('b');\n\
         const make = () => {\n\
           const cfg = { a: 1 }; delete cfg.a;\n\
           let m = 'a'; const x = env && (m = 'b');\n\
           let d = 'a'; ({ d } = options);\n\
           const items = ['a', 'b']; items.length = 0;\n\
           return { cfg, m, d, n: items.length };\n\
         };\n\
         export default [{ list }, make()];",
    );
    let configs = value.as_array().unwrap();
    assert_eq!(
        get(&configs[0], &["list"]),
        &ConfigValue::Array(vec![s("a"), ConfigValue::Unknown])
    );
    assert!(open(get(&configs[1], &["cfg"])));
    for key in ["m", "d", "n"] {
        assert_eq!(get(&configs[1], &[key]), &ConfigValue::Unknown, "{key}");
    }
}

#[test]
fn a_later_change_of_module_exports_is_not_lost() {
    let value = extract(
        "jest.config.js",
        "module.exports = { testMatch: ['a'] };\nmodule.exports.testMatch = ['b'];",
    );
    assert!(open(&value));
}

#[test]
fn a_branch_with_an_unknown_test_leaves_what_it_does_not_change_known() {
    let value = extract(
        "jest.config.js",
        "module.exports = async () => {\n\
           const config = { testMatch: ['**/*.spec.js'] };\n\
           const plugins = ['a'];\n\
           if (process.env.CI) { config.reporters = ['jest-junit']; plugins.push('b'); }\n\
           return { ...config, plugins };\n\
         };",
    );
    assert!(!open(&value));
    assert_eq!(get(&value, &["testMatch"]), &strings(&["**/*.spec.js"]));
    assert_eq!(get(&value, &["reporters"]), &ConfigValue::Unknown);
    assert_eq!(
        get(&value, &["plugins"]),
        &ConfigValue::Array(vec![s("a"), ConfigValue::Unknown])
    );
}

#[test]
fn a_function_in_a_config_does_not_share_what_its_parameters_take() {
    let value = extract(
        "next.config.js",
        "const nextConfig = {\n\
           transpilePackages: ['ui'],\n\
           webpack: (config) => { config.resolve.alias = {}; return config; },\n\
         };\nmodule.exports = nextConfig;",
    );
    assert!(!open(&value));
    assert_eq!(get(&value, &["transpilePackages"]), &strings(&["ui"]));
}

/// An unknown spread or merge of a value the config computes itself (a call of a function it declares, a constant it
/// computes, a parameter) is recorded apart from one of an imported value, a constant holding one, or a global.
#[test]
fn an_unknown_spread_the_config_computes_itself_is_its_own() {
    let value = extract(
        "vite.config.ts",
        "import shared from './shared';\n\
         import { mergeConfig } from 'vite';\n\
         function libEntries() { return JSON.parse(process.env.ENTRIES ?? '{}'); }\n\
         const makeLib = () => JSON.parse(process.env.LIB ?? '{}');\n\
         const computed = JSON.parse(process.env.BUILD ?? '{}');\n\
         const alias = shared.build;\n\
         const wrap = (options) => ({ ...options });\n\
         export default {\n\
           declared: { ...libEntries() },\n\
           arrow: { ...makeLib() },\n\
           constant: { ...computed },\n\
           parameter: wrap(unknown()),\n\
           merged: mergeConfig(shared, makeLib()),\n\
           imported: { ...shared },\n\
           aliased: { ...alias },\n\
           global: { ...JSON.parse(process.env.X ?? '{}') },\n\
         };",
    );
    for key in ["declared", "arrow", "constant", "parameter"] {
        let object = get(&value, &[key]);
        assert_eq!(object.get(OWN_SPREAD), Some(&ConfigValue::Unknown), "{key}: {object:?}");
        assert_eq!(object.get(UNKNOWN_SPREAD), None, "{key}: {object:?}");
    }
    let merged = get(&value, &["merged"]);
    assert!(
        merged.get(UNKNOWN_SPREAD).is_some() && merged.get(OWN_SPREAD).is_some(),
        "{merged:?}"
    );
    for key in ["imported", "aliased", "global"] {
        let object = get(&value, &[key]);
        assert_eq!(
            object.get(UNKNOWN_SPREAD),
            Some(&ConfigValue::Unknown),
            "{key}: {object:?}"
        );
        assert_eq!(object.get(OWN_SPREAD), None, "{key}: {object:?}");
    }
}

/// A function the tool calls (an exported config function, the callback of `defineConfig`, a declared function the
/// config exports by name) is given arguments the config cannot know: a parameter's default is only what it takes when
/// nothing is passed, so a test on it is unknown and a spread it guards adds its keys. A call in the config itself
/// passes what it passes, and a default fills what it leaves out.
#[test]
fn the_parameters_of_a_function_the_tool_calls_are_unknown() {
    let body = "({ entry: env.analyze ? './src/analyze.ts' : './src/index.ts', \
                inputs: { ...(env.analyze ? { analyze: ['./src/analyze.ts'] } : {}), index: './src/index.ts' } })";
    for source in [
        format!("export default function (env = {{}}) {{ return {body}; }}"),
        format!("export default defineConfig((env = {{}}) => {body});"),
        format!("module.exports = (env = {{}}) => {body};"),
        format!("function config(env = {{}}) {{ return {body}; }}\nexport default config;"),
    ] {
        let value = extract("webpack.config.js", &source);
        assert_eq!(
            get(&value, &["entry"]),
            &ConfigValue::OneOf(vec![s("./src/analyze.ts"), s("./src/index.ts")]),
            "{source}"
        );
        assert_eq!(
            get(&value, &["inputs", "analyze"]),
            &strings(&["./src/analyze.ts"]),
            "{source}"
        );
        assert_eq!(get(&value, &["inputs", "index"]), &s("./src/index.ts"), "{source}");
    }
    let called = extract(
        "webpack.config.js",
        &format!("const make = (env = {{}}) => {body};\nexport default make();"),
    );
    assert_eq!(get(&called, &["entry"]), &s("./src/index.ts"));
    assert_eq!(get(&called, &["inputs"]).get("analyze"), None);
}

/// A key a known object does not have reads as `undefined` alike through a member and through a destructuring, which
/// then takes its default; a key an unknown spread may set is unknown both ways.
#[test]
fn a_missing_key_is_undefined_to_a_member_and_a_destructuring_alike() {
    let value = extract(
        "vite.config.ts",
        "const pick = ({ mode }) => mode ?? 'fallback';\n\
         const defaulted = ({ mode = 'default' }) => mode;\n\
         const options = { base: '/' };\n\
         export default { member: options.mode ?? 'fallback', destructured: pick(options), \
         defaulted: defaulted(options), open: pick({ ...unknown() }), \
         openMember: { ...unknown() }.mode ?? 'fallback' };",
    );
    assert_eq!(get(&value, &["member"]), &s("fallback"));
    assert_eq!(get(&value, &["destructured"]), &s("fallback"));
    assert_eq!(get(&value, &["defaulted"]), &s("default"));
    assert_eq!(value.get("open"), Some(&ConfigValue::Unknown));
    assert_eq!(value.get("openMember"), Some(&ConfigValue::Unknown));
}

/// An optional member and a computed member whose key evaluates to a string read a known object as a member does. An
/// optional member of `null` or `undefined` is `undefined`, the rest of its chain unread, so a fallback over it
/// applies; a computed key that is not a known string leaves the value unknown.
#[test]
fn optional_and_computed_members_read_like_members() {
    let setup = strings(&["./setup.ts"]);
    let include = strings(&["src/a.test.ts"]);
    let cases = [
        ("base.setupFiles", setup.clone()),
        ("base?.setupFiles", setup.clone()),
        ("base['setupFiles']", setup.clone()),
        ("base?.['setupFiles']", setup.clone()),
        ("base[KEY]", setup.clone()),
        ("base[`setup${'Files'}`]", setup.clone()),
        ("base?.setupFiles ?? []", setup.clone()),
        ("base?.test.include", include.clone()),
        ("base.test?.['include']", include.clone()),
        ("base['test']?.include", include),
        ("missing?.setupFiles", ConfigValue::Null),
        ("missing?.a.b", ConfigValue::Null),
        ("base.absent?.setupFiles", ConfigValue::Null),
        ("missing?.setupFiles ?? ['./fallback.ts']", strings(&["./fallback.ts"])),
        ("base[process.env.KEY]", ConfigValue::Unknown),
        ("base.test.include[0]", ConfigValue::Unknown),
        ("opaque()?.setupFiles", ConfigValue::Unknown),
    ];
    for (read, expected) in cases {
        let source = format!(
            "const base = {{ setupFiles: ['./setup.ts'], test: {{ include: ['src/a.test.ts'] }} }};\n\
             const KEY = 'setupFiles';\nconst missing = null;\n\
             export default {{ value: {read} }};"
        );
        let value = extract("vitest.config.ts", &source);
        assert_eq!(get(&value, &["value"]), &expected, "{read}");
    }
}

/// A key a known object does not have itself may still be inherited: a member of `Object.prototype` (`constructor`,
/// `toString`, `hasOwnProperty`, `valueOf`) and any key of an object with a `__proto__` are unknown, to a member read
/// and a destructuring alike, rather than `undefined`.
#[test]
fn an_inherited_key_is_unknown_rather_than_undefined() {
    let value = extract(
        "vite.config.ts",
        "const options = { base: '/' };\n\
         const inherits = { __proto__: { mode: 'prod' }, base: '/' };\n\
         const pick = ({ toString }) => toString ?? 'fallback';\n\
         export default { constructor: options.constructor ?? 'fallback', toString: options.toString ?? 'fallback', \
         hasOwnProperty: options.hasOwnProperty ?? 'fallback', valueOf: options.valueOf ?? 'fallback', \
         proto: inherits.mode ?? 'fallback', destructured: pick(options), missing: options.mode ?? 'fallback' };",
    );
    for key in [
        "constructor",
        "toString",
        "hasOwnProperty",
        "valueOf",
        "proto",
        "destructured",
    ] {
        assert_eq!(value.get(key), Some(&ConfigValue::Unknown), "{key}: {value:?}");
    }
    assert_eq!(get(&value, &["missing"]), &s("fallback"));
}

/// A key merged from the branches of a spread whose test is unknown may be absent or hold any one of the values its
/// union lists, so a read of it is not definite: a fallback (`x.setupFiles ?? ['./default.ts']`, `||`, a destructuring
/// default) is credited too, a test on it is unknown, and so is its `length`, also through a nested object and a
/// spread of the object. A key set after the spread is definite again, and the exported object keeps only its keys.
#[test]
fn a_key_merged_from_undecided_branches_is_not_definite() {
    let value = extract(
        "vitest.config.ts",
        "const x = { ...(process.env.CI ? { setupFiles: ['./a.ts'], test: { include: ['a'] } } : {}) };\n\
         const y = { ...(process.env.CI ? { setupFiles: ['./a.ts'] } : {}), setupFiles: ['./b.ts'] };\n\
         const pick = ({ setupFiles = ['./d.ts'] }) => setupFiles;\n\
         export default {\n\
           coalesced: x.setupFiles ?? ['./default.ts'],\n\
           or: x.setupFiles || ['./or.ts'],\n\
           destructured: pick(x),\n\
           nested: x.test.include ?? ['b'],\n\
           respread: { ...x }.setupFiles ?? ['./default.ts'],\n\
           length: x.setupFiles.length,\n\
           fallbackLength: (x.setupFiles ?? []).length,\n\
           tested: x.setupFiles ? 1 : 2,\n\
           negated: !x.setupFiles,\n\
           set: y.setupFiles ?? ['./default.ts'],\n\
           setLength: y.setupFiles.length,\n\
           exported: x,\n\
         };",
    );
    assert_eq!(get(&value, &["coalesced"]), &strings(&["./a.ts", "./default.ts"]));
    assert_eq!(get(&value, &["or"]), &strings(&["./a.ts", "./or.ts"]));
    assert_eq!(get(&value, &["destructured"]), &strings(&["./a.ts", "./d.ts"]));
    assert_eq!(get(&value, &["nested"]), &strings(&["a", "b"]));
    assert_eq!(get(&value, &["respread"]), &strings(&["./a.ts", "./default.ts"]));
    for key in ["length", "fallbackLength", "negated"] {
        assert_eq!(value.get(key), Some(&ConfigValue::Unknown), "{key}: {value:?}");
    }
    assert_eq!(
        value.get("tested"),
        Some(&ConfigValue::OneOf(vec![
            ConfigValue::Number(1.0),
            ConfigValue::Number(2.0)
        ]))
    );
    assert_eq!(get(&value, &["set"]), &strings(&["./b.ts"]));
    assert_eq!(get(&value, &["setLength"]), &ConfigValue::Number(1.0));
    assert_eq!(
        get(&value, &["exported"]),
        &ConfigValue::Object(vec![
            ("setupFiles".to_owned(), strings(&["./a.ts"])),
            (
                "test".to_owned(),
                ConfigValue::Object(vec![("include".to_owned(), strings(&["a"]))])
            ),
        ])
    );
}

/// A key is an ordinary key whatever its name: one that reads like evaluator bookkeeping (`'\0maybe:a'`) is exported as
/// written and leaves a read of the key it seems to name definite.
#[test]
fn a_key_named_like_bookkeeping_is_an_ordinary_key() {
    let value = extract(
        "vitest.config.ts",
        "const x = { a: 'ab', '\\0maybe:a': 1 };\n\
         export default { exported: x, coalesced: x.a ?? 'fallback', length: x.a.length };",
    );
    assert_eq!(
        get(&value, &["exported"]),
        &ConfigValue::Object(vec![
            ("a".to_owned(), s("ab")),
            ("\0maybe:a".to_owned(), ConfigValue::Number(1.0)),
        ])
    );
    assert_eq!(get(&value, &["coalesced"]), &s("ab"));
    assert_eq!(get(&value, &["length"]), &ConfigValue::Number(2.0));
}

/// A constant holding a read that is not definite is not definite either, at the top level and in a function: a
/// fallback is credited too, and a test on it and its `length` are unknown.
#[test]
fn a_constant_holding_a_read_that_is_not_definite_is_not_definite() {
    let value = extract(
        "vitest.config.ts",
        "const x = { ...(process.env.CI ? { setupFiles: ['./a.ts'] } : {}) };\n\
         const files = x.setupFiles;\n\
         const local = () => {\n\
           const held = x.setupFiles;\n\
           return { coalesced: held ?? ['./b.ts'], length: held.length };\n\
         };\n\
         export default {\n\
           coalesced: files ?? ['./default.ts'],\n\
           or: files || ['./or.ts'],\n\
           length: files.length,\n\
           tested: files ? 1 : 2,\n\
           kept: files,\n\
           local: local(),\n\
         };",
    );
    assert_eq!(get(&value, &["coalesced"]), &strings(&["./a.ts", "./default.ts"]));
    assert_eq!(get(&value, &["or"]), &strings(&["./a.ts", "./or.ts"]));
    assert_eq!(get(&value, &["kept"]), &strings(&["./a.ts"]));
    assert_eq!(get(&value, &["local", "coalesced"]), &strings(&["./a.ts", "./b.ts"]));
    assert_eq!(value.get("length"), Some(&ConfigValue::Unknown), "{value:?}");
    assert_eq!(
        value.get("tested"),
        Some(&ConfigValue::OneOf(vec![
            ConfigValue::Number(1.0),
            ConfigValue::Number(2.0)
        ])),
        "{value:?}"
    );
    assert_eq!(
        value.get("local").and_then(|local| local.get("length")),
        Some(&ConfigValue::Unknown),
        "{value:?}"
    );
}

/// A read that is not definite stays so through every value it reaches: a key of an object (spread, merged or set on
/// a local), the argument of a call, the value a function returns and a local it is assigned to. A fallback after it is
/// credited too, so both the file a branch sets and the fallback stay named.
#[test]
fn a_read_that_is_not_definite_stays_so_through_every_value_it_reaches() {
    let base = "const base = { ...(process.env.CI ? { setupFiles: ['./ci.ts'] } : {}) };\n";
    for source in [
        "const t = { ...base };\nexport default { test: { setupFiles: t.setupFiles ?? ['./local.ts'] } };",
        "const t = Object.assign({}, base);\nexport default { test: { setupFiles: t.setupFiles ?? ['./local.ts'] } };",
        "const t = { files: base.setupFiles };\nexport default { test: { setupFiles: t.files ?? ['./local.ts'] } };",
        "const t = Object.assign({}, { files: base.setupFiles });\n\
         export default { test: { setupFiles: t.files ?? ['./local.ts'] } };",
        "const pick = (o) => o.setupFiles;\nexport default { test: { setupFiles: pick(base) ?? ['./local.ts'] } };",
        "function pick(o) { const files = o.setupFiles; return files; }\n\
         export default { test: { setupFiles: pick(base) ?? ['./local.ts'] } };",
        "const make = (files) => files ?? ['./local.ts'];\n\
         export default { test: { setupFiles: make(base.setupFiles) } };",
        "const make = () => { let files; files = base.setupFiles; return files ?? ['./local.ts']; };\n\
         export default { test: { setupFiles: make() } };",
    ] {
        let source = format!("{base}{source}");
        let value = extract("vitest.config.ts", &source);
        assert_eq!(
            get(&value, &["test", "setupFiles"]),
            &strings(&["./ci.ts", "./local.ts"]),
            "{source}"
        );
    }
    // A nested function that stores a part of `base` into an object it changes may share that part with `base`
    // (`sharing.rs`), so the read is unknown rather than the union.
    let shared = format!(
        "{base}const make = () => {{ const t = {{}}; t.files = base.setupFiles; return t.files ?? ['./local.ts']; }};\n\
         export default {{ test: {{ setupFiles: make() }} }};"
    );
    assert_eq!(
        get(&extract("vitest.config.ts", &shared), &["test", "setupFiles"]),
        &ConfigValue::Unknown
    );
    let spread = extract(
        "vitest.config.ts",
        &format!(
            "{base}const files = [...(base.setupFiles ?? [])];\n\
             export default {{ test: {{ setupFiles: files.length ? files : ['./local.ts'], length: files.length }} }};"
        ),
    );
    assert_eq!(
        get(&spread, &["test", "setupFiles"]),
        &ConfigValue::OneOf(vec![strings(&["./ci.ts"]), strings(&["./local.ts"])]),
        "{spread:?}"
    );
    assert_eq!(get(&spread, &["test", "length"]), &ConfigValue::Unknown, "{spread:?}");
}

/// A key an object does not have is `undefined`, so a parameter's or a destructuring's default applies to it, while a
/// literal `null` skips the default; `??` falls back on both, and an absent key is exported as `null`.
#[test]
fn an_absent_key_takes_a_default_and_null_does_not() {
    let value = extract(
        "vitest.config.ts",
        "const base = { test: { include: ['a'] } };\n\
         function make(setupFiles = ['./setup.ts']) { return setupFiles; }\n\
         const absent = () => { const o = { f: base.x }; const { f = ['./d.ts'] } = o; return f; };\n\
         const nulled = () => { const o = { f: null }; const { f = ['./d.ts'] } = o; return f; };\n\
         export default {\n\
           parameter: make(base.test.setupFiles),\n\
           destructured: absent(),\n\
           nullParameter: make(null),\n\
           nullDestructured: nulled(),\n\
           coalesced: base.test.setupFiles ?? ['./c.ts'],\n\
           nullCoalesced: null ?? ['./n.ts'],\n\
           exported: { f: base.x },\n\
         };",
    );
    assert_eq!(get(&value, &["parameter"]), &strings(&["./setup.ts"]));
    assert_eq!(get(&value, &["destructured"]), &strings(&["./d.ts"]));
    assert_eq!(get(&value, &["nullParameter"]), &ConfigValue::Null);
    assert_eq!(get(&value, &["nullDestructured"]), &ConfigValue::Null);
    assert_eq!(get(&value, &["coalesced"]), &strings(&["./c.ts"]));
    assert_eq!(get(&value, &["nullCoalesced"]), &strings(&["./n.ts"]));
    assert_eq!(
        get(&value, &["exported"]),
        &ConfigValue::Object(vec![("f".to_owned(), ConfigValue::Null)])
    );
}

/// The arguments kept for an exported call that cannot be followed are the values the evaluation computed, not a
/// second evaluation, which would find the call budget spent by what the function did after its arguments.
#[test]
fn the_arguments_of_an_unknown_call_are_the_ones_evaluated() {
    let source = "const files = () => ['./config/boot.ts'];\n\
                  const heavy = (n) => [heavy(n), heavy(n)];\n\
                  function createConfig(options) { const spent = heavy(1); return build(options); }\n\
                  export default createConfig({ setupFiles: files() });";
    let found = extract_config(source, Path::new("vitest.config.ts"), None).expect("config is understood");
    assert_eq!(found.value, ConfigValue::Unknown);
    assert_eq!(
        found.call_arguments,
        vec![ConfigValue::Object(vec![(
            "setupFiles".to_owned(),
            strings(&["./config/boot.ts"])
        )])]
    );
}

/// The arguments of a call that cannot be followed are kept also when a config function the tool calls returns it,
/// its parameters unknown whatever top-level binding they shadow, and when a known merge is given its result, directly
/// or through a constant, next to the known options the merge applies.
#[test]
fn the_arguments_of_an_unknown_call_returned_or_merged_are_kept() {
    let options = ConfigValue::Object(vec![("setupFiles".to_owned(), strings(&["./s.ts"]))]);
    let extract = |source: &str| {
        let found = extract_config(source, Path::new("vitest.config.ts"), None).expect("config is understood");
        (found.value, found.call_arguments)
    };
    let preset = "import { defineConfig, mergeConfig } from 'vitest/config';\n\
                  import { createConfig, createBase } from '@acme/vitest-preset';\n";
    for returned in [
        "export default () => createConfig({ setupFiles: ['./s.ts'] });",
        "export default function () { return createConfig({ setupFiles: ['./s.ts'] }); }",
        "export default defineConfig(() => createConfig({ setupFiles: ['./s.ts'] }));",
    ] {
        let source = format!("{preset}{returned}");
        assert_eq!(
            extract(&source),
            (ConfigValue::Unknown, vec![options.clone()]),
            "{source}"
        );
    }
    let parameter = format!(
        "{preset}const env = {{ mode: 'production' }};\n\
         export default (env) => createConfig({{ setupFiles: ['./s.ts'], mode: env.mode }});"
    );
    assert_eq!(
        extract(&parameter).1,
        vec![ConfigValue::Object(vec![
            ("setupFiles".to_owned(), strings(&["./s.ts"])),
            ("mode".to_owned(), ConfigValue::Unknown),
        ])]
    );
    for merged in [
        "export default mergeConfig(createBase({ setupFiles: ['./s.ts'] }), { test: { environment: 'jsdom' } });",
        "const base = createBase({ setupFiles: ['./s.ts'] });\n\
         export default mergeConfig(base, { test: { environment: 'jsdom' } });",
        "export default defineConfig(mergeConfig(createBase({ setupFiles: ['./s.ts'] }), \
         { test: { environment: 'jsdom' } }));",
    ] {
        let source = format!("{preset}{merged}");
        let (value, arguments) = extract(&source);
        assert_eq!(get(&value, &["test", "environment"]), &s("jsdom"), "{source}");
        assert_eq!(value.get(UNKNOWN_SPREAD), Some(&ConfigValue::Unknown), "{source}");
        assert_eq!(arguments, vec![options.clone()], "{source}");
    }
    let known = format!("{preset}export default mergeConfig({{ test: {{ setupFiles: ['./s.ts'] }} }}, {{}});");
    assert_eq!(extract(&known).1, Vec::new());
}

/// A call that cannot be followed nested in an object or array literal of the exported config keeps its arguments, each
/// in an object under the keys it is found at, so a key that excludes files still holds what it names. A spread keeps
/// them when the object has an unknown spread; a call the evaluator follows, and one after a spread into an array, keep
/// none.
#[test]
fn the_arguments_of_an_unknown_call_nested_in_the_config_are_kept_under_their_keys() {
    let arguments = |source: &str| {
        let source = format!(
            "import {{ defineConfig }} from 'vitest/config';\n\
             import {{ createTest, makeList, plugin, base }} from '@acme/vitest-preset';\n{source}"
        );
        extract_config(&source, Path::new("vitest.config.ts"), None)
            .expect("config is understood")
            .call_arguments
    };
    let under = |key: &str, value: ConfigValue| ConfigValue::Object(vec![(key.to_owned(), value)]);
    let setup = under("setupFiles", strings(&["./setup.ts"]));
    let cases = [
        (
            "export default { test: createTest({ setupFiles: ['./setup.ts'] }) };",
            vec![under("test", setup.clone())],
        ),
        (
            "export default defineConfig({ test: { coverage: { exclude: makeList(['./skip.ts']) } } });",
            vec![under(
                "test",
                under("coverage", under("exclude", strings(&["./skip.ts"]))),
            )],
        ),
        (
            "export default { plugins: ['a', plugin({ config: './plugin.config.ts' })] };",
            vec![under("plugins", under("config", s("./plugin.config.ts")))],
        ),
        (
            "export default { ...createTest({ setupFiles: ['./setup.ts'] }) };",
            vec![setup],
        ),
        (
            "const files = () => ['./setup.ts'];\nexport default { test: { setupFiles: files() } };",
            Vec::new(),
        ),
        (
            "export default { plugins: [...base(), plugin('./after.ts')] };",
            Vec::new(),
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(arguments(source), expected, "{source}");
    }
}

/// A condition the evaluator cannot decide between values it knows is one of them: a ternary, a fallback over a read
/// that is not definite and a binding one branch reassigns. A branch it does not know leaves the value unknown, and so
/// do more alternatives than it keeps.
#[test]
fn an_undecided_condition_between_known_values_is_one_of_them() {
    let value = extract(
        "playwright.config.ts",
        "const x = { reporter: 'list', ...(process.env.CI ? { reporter: 'github' } : {}) };\n\
         const make = () => { let mode = 'dev'; if (process.env.CI) { mode = 'ci'; } return mode; };\n\
         export default {\n\
           reporter: process.env.CI ? 'github' : 'list',\n\
           listed: [process.env.CI ? 'blob' : 'list'],\n\
           same: process.env.CI ? 'list' : 'list',\n\
           nested: process.env.A ? 'a' : process.env.B ? 'b' : 'a',\n\
           mixed: process.env.CI ? [['html']] : 'list',\n\
           optional: process.env.CI ? 'github' : undefined,\n\
           nullish: process.env.CI ? null : undefined,\n\
           objects: process.env.CI ? { reporter: 'github', retries: 2 } : { reporter: 'list', retries: 2 },\n\
           fallback: x.reporter ?? 'dot',\n\
           reassigned: make(),\n\
           opaque: process.env.CI ? 'github' : helper(),\n\
           many: process.env.A ? 'a' : process.env.B ? 'b' : process.env.C ? 'c' : process.env.D ? 'd' \
             : process.env.E ? 'e' : process.env.F ? 'f' : process.env.G ? 'g' : process.env.H ? 'h' : 'i',\n\
         };",
    );
    let pairs = [
        ("reporter", ConfigValue::OneOf(vec![s("github"), s("list")])),
        (
            "listed",
            ConfigValue::Array(vec![ConfigValue::OneOf(vec![s("blob"), s("list")])]),
        ),
        ("same", s("list")),
        ("nested", ConfigValue::OneOf(vec![s("a"), s("b")])),
        (
            "mixed",
            ConfigValue::OneOf(vec![ConfigValue::Array(vec![strings(&["html"])]), s("list")]),
        ),
        ("optional", ConfigValue::OneOf(vec![s("github"), ConfigValue::Null])),
        ("nullish", ConfigValue::Null),
        ("fallback", ConfigValue::OneOf(vec![s("github"), s("list"), s("dot")])),
        ("reassigned", ConfigValue::OneOf(vec![s("dev"), s("ci")])),
        ("opaque", ConfigValue::Unknown),
        ("many", ConfigValue::Unknown),
    ];
    for (key, expected) in pairs {
        assert_eq!(get(&value, &[key]), &expected, "{key}");
    }
    assert_eq!(
        get(&value, &["objects"]),
        &ConfigValue::Object(vec![
            ("reporter".to_owned(), ConfigValue::OneOf(vec![s("github"), s("list")])),
            ("retries".to_owned(), ConfigValue::Number(2.0)),
        ])
    );
}

/// A spread of a value that is one of several objects adds the keys of each, as a spread of the condition itself does.
#[test]
fn a_spread_of_one_of_several_values_adds_the_keys_of_each() {
    let value = extract(
        "playwright.config.ts",
        "const extra = process.env.CI ? { reporter: 'github' } : undefined;\n\
         export default { reporter: 'list', ...extra };",
    );
    assert_eq!(value.get(OWN_SPREAD), None, "{value:?}");
    assert_eq!(
        get(&value, &["reporter"]),
        &ConfigValue::OneOf(vec![s("github"), s("list")])
    );
}

/// An array spreads the elements of every branch of a condition it cannot decide, or of a value that is one of several
/// arrays, a branch that adds nothing (`[]`, `null`, a falsy left side of `&&`) included; a branch it does not know
/// adds an unknown element.
#[test]
fn an_array_spreads_the_elements_of_every_branch() {
    let value = extract(
        "playwright.config.ts",
        "const extra = process.env.CI ? ['a'] : ['b'];\n\
         export default {\n\
           reporter: [['list'], ...(process.env.A11Y ? [['./reporters/a11y-reporter.ts']] : [])],\n\
           both: [...(process.env.CI ? ['a'] : ['b', 'a'])],\n\
           guarded: [...(process.env.CI && ['x'])],\n\
           held: [...extra],\n\
           opaque: [...(process.env.CI ? ['a'] : helper())],\n\
           length: [...(process.env.CI ? ['a'] : [])].length,\n\
         };",
    );
    assert_eq!(
        get(&value, &["reporter"]),
        &ConfigValue::Array(vec![strings(&["list"]), strings(&["./reporters/a11y-reporter.ts"])])
    );
    assert_eq!(get(&value, &["both"]), &strings(&["a", "b"]));
    assert_eq!(get(&value, &["guarded"]), &strings(&["x"]));
    assert_eq!(get(&value, &["held"]), &strings(&["a", "b"]));
    assert_eq!(
        get(&value, &["opaque"]),
        &ConfigValue::Array(vec![ConfigValue::Unknown])
    );
    assert_eq!(get(&value, &["length"]), &ConfigValue::Unknown);
}

/// `.filter(Boolean)` of a known array drops what is falsy and keeps an element that may be falsy as the values it
/// has when it is not; its length is then unknown. A `Boolean` the module binds itself is not the global.
#[test]
fn filter_boolean_keeps_what_may_be_truthy() {
    let value = extract(
        "playwright.config.ts",
        "export default {\n\
           reporter: [process.env.CI ? ['github'] : null, ['list']].filter(Boolean),\n\
           guarded: [process.env.CI && 'blob', 'list', false, null, '', 0, undefined].filter(Boolean),\n\
           optional: [process.env.CI ? 'github' : undefined].filter(Boolean),\n\
           opaque: [helper()].filter(Boolean),\n\
           maybe: [process.env.CI && 'x'].filter(Boolean).length,\n\
           known: ['a', null].filter(Boolean).length,\n\
         };",
    );
    assert_eq!(
        get(&value, &["reporter"]),
        &ConfigValue::Array(vec![strings(&["github"]), strings(&["list"])])
    );
    assert_eq!(get(&value, &["guarded"]), &strings(&["blob", "list"]));
    assert_eq!(get(&value, &["optional"]), &strings(&["github"]));
    assert_eq!(
        get(&value, &["opaque"]),
        &ConfigValue::Array(vec![ConfigValue::Unknown])
    );
    assert_eq!(get(&value, &["maybe"]), &ConfigValue::Unknown);
    assert_eq!(get(&value, &["known"]), &ConfigValue::Number(1.0));

    let shadowed = extract(
        "playwright.config.ts",
        "const Boolean = (value) => !value;\nexport default { reporter: [null, 'list'].filter(Boolean) };",
    );
    assert_eq!(get(&shadowed, &["reporter"]), &ConfigValue::Unknown);
}
