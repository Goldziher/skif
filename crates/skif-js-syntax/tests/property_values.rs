//! Values of a property wherever it appears, for plugins that read options written anywhere in a config.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ConfigValue, extract_property_calls, extract_property_values};

fn s(v: &str) -> ConfigValue {
    ConfigValue::String(v.to_string())
}

#[test]
fn every_property_with_the_key_is_evaluated_in_source_order() {
    let source = r#"
        const api = "src/api.handler";
        export default $config({
          app() { return { name: "x" }; },
          async run() {
            new sst.aws.Function("Api", { handler: api });
            new sst.aws.Function("Cron", { handler: ["src/a.run", "src/b.run"], nested: { handler: computed() } });
            const handler = () => {};
            return { handler() {} };
          },
        });
    "#;
    let found = extract_property_values(source, Path::new("sst.config.ts"), "handler");
    assert_eq!(
        found,
        [
            s("src/api.handler"),
            ConfigValue::Array(vec![s("src/a.run"), s("src/b.run")]),
            ConfigValue::Unknown,
        ]
    );
}

#[test]
fn quoted_keys_count_and_unparsable_files_yield_nothing() {
    let found = extract_property_values(r#"x({ "srcDir": "app" })"#, Path::new("a.js"), "srcDir");
    assert_eq!(found, [s("app")]);
    assert_eq!(extract_property_values("x({", Path::new("a.js"), "srcDir"), []);
}

fn object(entries: &[(&str, ConfigValue)]) -> ConfigValue {
    ConfigValue::Object(entries.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect())
}

#[test]
fn function_properties_are_called_with_the_argument() {
    let source = r#"
        module.exports = {
          configureWebpack(config) { return { entry: "./src/method.ts" }; },
          other: { configureWebpack: (config) => { config.entry = { admin: "./src/admin.ts" }; } },
          third: { configureWebpack: (config) => config },
          fourth: { configureWebpack: { entry: "./src/object.ts" } },
        };
    "#;
    let argument = object(&[("mode", s("development")), ("devtool", s("source-map"))]);
    let found = extract_property_calls(source, Path::new("vue.config.js"), "configureWebpack", &argument);
    assert_eq!(
        found,
        [
            object(&[("entry", s("./src/method.ts"))]),
            object(&[
                ("mode", s("development")),
                ("devtool", s("source-map")),
                ("entry", object(&[("admin", s("./src/admin.ts"))])),
            ]),
            argument.clone(),
        ]
    );
    assert_eq!(
        extract_property_calls("x({", Path::new("a.js"), "configureWebpack", &argument),
        []
    );
}
