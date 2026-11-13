//! Cucumber: the step definitions and support code of `import` and `require`, and the custom formatters of `format`,
//! in every profile of `cucumber.*`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, module_input, strings};

/// The `cucumber` plugin.
pub struct Cucumber;

const DEFAULT_IMPORT: &str = "features/**/*.{js,cjs,mjs}";

static SPEC: PluginSpec = PluginSpec {
    id: "cucumber",
    enablers: &[Enabler::Dependency("@cucumber/cucumber")],
    config: &["cucumber.{json,yaml,yml,js,cjs,mjs}"],
    entry: &[DEFAULT_IMPORT],
    binaries: &["cucumber-js"],
    ..PluginSpec::DEFAULT
};

const BUILTIN_FORMATTERS: &[&str] = &[
    "html",
    "json",
    "junit",
    "message",
    "pretty",
    "progress",
    "progress-bar",
    "rerun",
    "snippets",
    "summary",
    "usage",
    "usage-json",
];

impl Plugin for Cucumber {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// A JSON or YAML config, or one with a `default` key, is an object of profiles; a JavaScript config is one
    /// profile.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let structured = [".json", ".yaml", ".yml"]
            .iter()
            .any(|extension| config.file_name().ends_with(extension));
        let profiles: Vec<&ConfigValue> = match value {
            ConfigValue::Object(entries) if structured || value.get("default").is_some() => {
                entries.iter().map(|(_, profile)| profile).collect()
            }
            other => vec![other],
        };
        let mut out = Vec::new();
        for profile in profiles {
            let imports = profile.get("import").map_or_else(|| vec![DEFAULT_IMPORT], strings);
            let requires = profile.get("require").map_or_default(strings);
            for pattern in imports.into_iter().chain(requires) {
                out.extend(entry_glob(config.base, pattern).map(PluginInput::Entry));
            }
            for format in profile
                .get("format")
                .and_then(ConfigValue::as_array)
                .unwrap_or_default()
            {
                let Some(name) = formatter_name(format) else { continue };
                if !BUILTIN_FORMATTERS.contains(&name) {
                    out.extend(module_input(config.base, name, false));
                }
            }
        }
        out
    }
}

/// The formatter of a `format` item: `"name"`, `"name:output"`, `"\"name\":output"` or `[name, output]`.
fn formatter_name(format: &ConfigValue) -> Option<&str> {
    match format {
        ConfigValue::Array(items) => items.first()?.as_str(),
        ConfigValue::String(text) => {
            if let Some(quoted) = text.strip_prefix('"') {
                return quoted.split_once('"').map(|(name, _)| name);
            }
            text.split(':').next()
        }
        _ => None,
    }
}
