//! webpack: entries and the loaders named in `module.rules`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::babel;
use crate::support::{alias_inputs, get_in, module_input, objects};

/// The `webpack` plugin.
pub struct Webpack;

static SPEC: PluginSpec = PluginSpec {
    id: "webpack",
    enablers: &[Enabler::Dependency("webpack"), Enabler::Dependency("webpack-cli")],
    config: &["webpack.config.{js,ts,mjs,cjs,mts,cts}"],
    binaries: &["webpack", "webpack-cli"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Webpack {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for object in objects(config.value) {
            if let Some(entry) = object.get("entry") {
                entries(entry, config, &mut out);
            }
            if let Some(rules) = object.get("module").and_then(|m| m.get("rules")) {
                rule_loaders(rules, config, &mut out, 0);
            }
            if let Some(alias) = get_in(object, &["resolve", "alias"]) {
                out.extend(alias_inputs(alias, config));
            }
        }
        out
    }
}

/// `entry` is a path, a list of paths, or an object whose values are either of those or `{ import }`.
fn entries(entry: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    match entry {
        ConfigValue::String(path) => out.push(PluginInput::ProductionEntry(config.join(path))),
        ConfigValue::Array(items) => items.iter().for_each(|item| entries(item, config, out)),
        ConfigValue::Object(properties) => match entry.get("import") {
            Some(import) => entries(import, config, out),
            None => properties.iter().for_each(|(_, value)| entries(value, config, out)),
        },
        _ => {}
    }
}

fn rule_loaders(rules: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 6 {
        return;
    }
    for rule in objects(rules) {
        for key in ["use", "loader"] {
            for item in rule.get(key).map(items).unwrap_or_default() {
                loader(item, rule.get("options"), config, out);
            }
        }
        for key in ["oneOf", "rules"] {
            if let Some(nested) = rule.get(key) {
                rule_loaders(nested, config, out, depth + 1);
            }
        }
    }
}

/// A loader is a string or `{ loader, options }`; the rule's own `options` apply to a string `loader`.
fn loader(item: &ConfigValue, rule_options: Option<&ConfigValue>, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    let (name, options) = match item {
        ConfigValue::String(name) => (name.as_str(), rule_options),
        ConfigValue::Object(_) => match item.get("loader").and_then(ConfigValue::as_str) {
            Some(name) => (name, item.get("options")),
            None => return,
        },
        _ => return,
    };
    out.extend(loader_input(config, name));
    if name.split('?').next() == Some("babel-loader")
        && let Some(options) = options
    {
        out.extend(babel::dependencies(options, config.base, 0));
    }
}

fn loader_input(config: &ConfigAst<'_>, name: &str) -> Option<PluginInput> {
    module_input(config.base, name.split('?').next().unwrap_or_default(), false)
}

fn items(value: &ConfigValue) -> Vec<&ConfigValue> {
    match value {
        ConfigValue::Array(items) => items.iter().collect(),
        other => vec![other],
    }
}
