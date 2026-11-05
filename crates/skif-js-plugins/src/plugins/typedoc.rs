//! `TypeDoc`: plugins and themes are packages or local modules, custom CSS and JavaScript files are entries.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, package_name, strings};

/// The `typedoc` plugin.
pub struct Typedoc;

static SPEC: PluginSpec = PluginSpec {
    id: "typedoc",
    enablers: &[Enabler::Dependency("typedoc")],
    config: &[
        "typedoc.{js,cjs,mjs,json,jsonc}",
        "typedoc.config.{js,cjs,mjs}",
        ".config/typedoc.{js,cjs,mjs,json,jsonc}",
        ".config/typedoc.config.{js,cjs,mjs}",
        "tsconfig.json",
    ],
    package_json_key: Some("typedocOptions"),
    binaries: &["typedoc"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Typedoc {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let options = if config.file_name().starts_with("tsconfig") {
            config.value.get("typedocOptions")
        } else {
            Some(config.value)
        };
        let Some(options) = options else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for key in ["plugin", "theme"] {
            for name in options.get(key).map(strings).unwrap_or_default() {
                if is_relative(name) {
                    out.push(PluginInput::Entry(config.join(name)));
                } else {
                    out.extend(package_name(name).map(PluginInput::OptionalDependency));
                }
            }
        }
        for key in ["customCss", "customJs"] {
            if let Some(file) = options.get(key).and_then(ConfigValue::as_str) {
                out.push(PluginInput::Entry(config.join(file)));
            }
        }
        out
    }
}
