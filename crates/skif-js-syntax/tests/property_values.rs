//! Values of a property wherever it appears, for plugins that read options written anywhere in a config.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ConfigValue, extract_property_values};

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
