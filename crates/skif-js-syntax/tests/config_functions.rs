//! Static evaluation of config functions: local bindings, local calls, `await`, `require.resolve`, glob calls.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ConfigValue, extract_config_object};

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
