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
