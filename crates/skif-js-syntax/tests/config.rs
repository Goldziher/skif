//! Static config extraction.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ConfigValue, extract_config_object};

fn extract(path: &str, source: &str) -> Option<ConfigValue> {
    extract_config_object(source, Path::new(path))
}

fn s(v: &str) -> ConfigValue {
    ConfigValue::String(v.to_string())
}

fn obj(entries: Vec<(&str, ConfigValue)>) -> ConfigValue {
    ConfigValue::Object(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

#[test]
fn shapes() {
    let cases: Vec<(&str, &str, &str, Option<ConfigValue>)> = vec![
        (
            "plain export default",
            "c.js",
            "export default { a: 1, b: 'x', c: true, d: null, e: [1, -2], 'f-g': { h: `t` } };",
            Some(obj(vec![
                ("a", ConfigValue::Number(1.0)),
                ("b", s("x")),
                ("c", ConfigValue::Bool(true)),
                ("d", ConfigValue::Null),
                (
                    "e",
                    ConfigValue::Array(vec![ConfigValue::Number(1.0), ConfigValue::Number(-2.0)]),
                ),
                ("f-g", obj(vec![("h", s("t"))])),
            ])),
        ),
        (
            "vite defineConfig with satisfies",
            "vite.config.ts",
            "import { defineConfig } from 'vite';\nexport default defineConfig({ plugins: [react()], test: { setupFiles: ['./setup.ts'] } }) satisfies unknown;",
            Some(obj(vec![
                ("plugins", ConfigValue::Array(vec![ConfigValue::Unknown])),
                (
                    "test",
                    obj(vec![("setupFiles", ConfigValue::Array(vec![s("./setup.ts")]))]),
                ),
            ])),
        ),
        (
            "vite function form",
            "vite.config.ts",
            "export default defineConfig(({ mode }) => ({ base: '/app/' }));",
            Some(obj(vec![("base", s("/app/"))])),
        ),
        (
            "jest module.exports",
            "jest.config.js",
            "module.exports = { preset: 'ts-jest', testMatch: ['**/*.test.ts'], setupFiles: [`./a.js`] };",
            Some(obj(vec![
                ("preset", s("ts-jest")),
                ("testMatch", ConfigValue::Array(vec![s("**/*.test.ts")])),
                ("setupFiles", ConfigValue::Array(vec![s("./a.js")])),
            ])),
        ),
        (
            "eslint flat array with spread",
            "eslint.config.mjs",
            "const base = [{ files: ['a'] }];\nexport default [...base, { ignores: ['dist'] }];",
            Some(ConfigValue::Array(vec![
                obj(vec![("files", ConfigValue::Array(vec![s("a")]))]),
                obj(vec![("ignores", ConfigValue::Array(vec![s("dist")]))]),
            ])),
        ),
        (
            "identifier to top-level const and as const",
            "c.ts",
            "const config = { entry: ['src/index.ts'] } as const;\nexport default config;",
            Some(obj(vec![("entry", ConfigValue::Array(vec![s("src/index.ts")]))])),
        ),
        (
            "shorthand and spread of const object",
            "c.js",
            "const name = 'n';\nconst more = { z: 1 };\nexport default { name, ...more };",
            Some(obj(vec![("name", s("n")), ("z", ConfigValue::Number(1.0))])),
        ),
        (
            "dynamic parts are unknown",
            "c.js",
            "export default { a: process.env.X, b: foo(), c: `${x}`, ...rest, d() {}, e: () => 1 };",
            Some(obj(vec![
                ("a", ConfigValue::Unknown),
                ("b", ConfigValue::Unknown),
                ("c", ConfigValue::Unknown),
                ("...", ConfigValue::Unknown),
                ("d", ConfigValue::Unknown),
                ("e", ConfigValue::Number(1.0)),
            ])),
        ),
        (
            "let is not inlined",
            "c.js",
            "let c = {};\nexport default c;",
            Some(ConfigValue::Unknown),
        ),
        ("no export", "c.js", "const c = {};", None),
        ("syntax error", "c.js", "export default {", None),
        ("unknown extension", "c.txt", "export default {}", None),
        (
            "cyclic constants stay finite",
            "c.js",
            "const a = b; const b = a; export default a;",
            Some(ConfigValue::Unknown),
        ),
    ];
    for (name, path, source, expected) in cases {
        assert_eq!(extract(path, source), expected, "{name}");
    }
}

#[test]
fn accessors() {
    let value = extract("c.js", "export default { a: { b: ['x'] }, c: true, d: foo() };").unwrap();
    assert_eq!(
        value
            .get("a")
            .and_then(|a| a.get("b"))
            .and_then(ConfigValue::as_array)
            .map(<[_]>::len),
        Some(1)
    );
    assert_eq!(value.get("c").and_then(ConfigValue::as_bool), Some(true));
    assert!(value.has_unknown());
    assert!(!value.get("a").unwrap().has_unknown());
}

#[test]
fn multibyte_strings_survive() {
    let value = extract("c.js", "export default { name: 'héllo→' };").unwrap();
    assert_eq!(value.get("name").and_then(ConfigValue::as_str), Some("héllo→"));
}

#[test]
fn computed_paths_are_relative_to_the_config_directory() {
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "namespace import of path",
            "c.mjs",
            "import path from 'node:path';\nexport default { a: path.resolve(import.meta.dirname, 'src', '..', 'lib') };",
            "./lib",
        ),
        (
            "required path and __dirname",
            "c.js",
            "const path = require('path');\nmodule.exports = { a: path.resolve(__dirname, 'src') };",
            "./src",
        ),
        (
            "destructured require",
            "c.cjs",
            "const { resolve, join: j } = require('node:path');\nmodule.exports = { a: resolve(__dirname, '../shared/js'), b: j(__dirname, 'a', 'b') };",
            "../shared/js",
        ),
        (
            "named import and a dirname constant",
            "c.mjs",
            "import { dirname, resolve } from 'path';\nimport { fileURLToPath } from 'url';\nconst __dirname = dirname(fileURLToPath(import.meta.url));\nexport default { a: resolve(__dirname, 'src') };",
            "./src",
        ),
        (
            "URL against import.meta.url",
            "c.mjs",
            "import { fileURLToPath } from 'node:url';\nexport default { a: fileURLToPath(new URL('./src', import.meta.url)) };",
            "./src",
        ),
        (
            "URL of the directory itself",
            "c.mjs",
            "export default { a: new URL('.', import.meta.url).pathname };",
            ".",
        ),
        (
            "template literal and concatenation",
            "c.js",
            "module.exports = { a: `${__dirname}/src/app`, b: __dirname + '/lib' };",
            "./src/app",
        ),
        (
            "constant holding a path",
            "c.js",
            "const path = require('path');\nconst src = path.join(__dirname, 'src');\nmodule.exports = { a: path.resolve(src, 'x') };",
            "./src/x",
        ),
    ];
    for (name, file, source, expected) in cases {
        let value = extract(file, source).unwrap();
        assert_eq!(value.get("a"), Some(&s(expected)), "{name}");
    }
    let value = extract(
        "c.cjs",
        "const { join: j } = require('node:path');\nmodule.exports = { b: j(__dirname, 'a', 'b'), c: `${__dirname}/lib`, d: __dirname + '/lib' };",
    )
    .unwrap();
    assert_eq!(value.get("b"), Some(&s("./a/b")));
    assert_eq!(value.get("c"), Some(&s("./lib")));
    assert_eq!(value.get("d"), Some(&s("./lib")));
}

#[test]
fn paths_that_cannot_be_known_stay_unknown() {
    let value = extract(
        "c.js",
        "const path = require('path');\nmodule.exports = { a: path.resolve('/abs', 'x'), b: path.resolve(__dirname, process.env.DIR), c: Promise.resolve(1), d: other.join(__dirname, 'x') };",
    )
    .unwrap();
    for key in ["a", "b", "c", "d"] {
        assert_eq!(value.get(key), Some(&ConfigValue::Unknown), "{key}");
    }
}

fn leaves(value: &ConfigValue) -> usize {
    match value {
        ConfigValue::Array(items) => items.iter().map(leaves).sum(),
        _ => 1,
    }
}

/// `const a1 = ['x']; const a2 = [a1, a1]; …` up to `a{depth}`, exported.
fn doubling_constants(depth: usize) -> String {
    let mut source = String::from("const a1 = ['x'];\n");
    for level in 2..=depth {
        source.push_str(&format!("const a{level} = [a{prev}, a{prev}];\n", prev = level - 1));
    }
    source.push_str(&format!("export default {{ v: a{depth} }};"));
    source
}

#[test]
fn constants_that_double_per_level_are_evaluated_once_and_bounded() {
    let small = extract("c.js", &doubling_constants(10)).unwrap();
    assert_eq!(leaves(small.get("v").unwrap()), 512);

    let huge = extract("c.js", &doubling_constants(30)).unwrap();
    assert!(huge.get("v").unwrap().has_unknown());
}

#[test]
fn long_constant_chains_and_cycles_terminate() {
    let mut source = String::from("const c1 = 'x';\n");
    for level in 2..=30 {
        source.push_str(&format!("const c{level} = c{};\n", level - 1));
    }
    source.push_str("const p = [q];\nconst q = [p];\nexport default { v: c30, cycle: p };");
    let value = extract("c.js", &source).unwrap();
    assert_eq!(value.get("v"), Some(&s("x")));
    assert!(value.get("cycle").unwrap().has_unknown());
}
