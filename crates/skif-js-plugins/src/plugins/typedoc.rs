//! `TypeDoc`: plugins and themes are packages or local modules, custom CSS and JavaScript files are entries. The
//! values of `--plugin` and `--theme` on the command line are plugins and themes too.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::flag_values;
use crate::support::{is_relative, join_rel, package_name, strings};

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
            for name in options.get(key).map_or_default(strings) {
                out.extend(module_ref(&config.join(name), name));
            }
        }
        for key in ["customCss", "customJs"] {
            if let Some(file) = options.get(key).and_then(ConfigValue::as_str) {
                out.push(PluginInput::Entry(config.join(file)));
            }
        }
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        cx.scripts
            .values()
            .flat_map(|script| flag_values(script, SPEC.binaries, &["--plugin", "--theme"]))
            .filter_map(|name| module_ref(&join_rel("", &name), &name))
            .collect()
    }
}

/// A plugin or theme: a local module is an entry (`path` is where it sits), anything else a package that may be absent.
fn module_ref(path: &str, name: &str) -> Option<PluginInput> {
    if is_relative(name) {
        Some(PluginInput::Entry(path.to_owned()))
    } else {
        package_name(name).map(PluginInput::OptionalDependency)
    }
}
