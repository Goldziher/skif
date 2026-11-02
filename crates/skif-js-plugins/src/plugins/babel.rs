//! Babel: presets and plugins named by string follow Babel's package naming conventions.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, join_rel, name_of, objects, package_name, strings};

/// The `babel` plugin.
pub struct Babel;

static SPEC: PluginSpec = PluginSpec {
    id: "babel",
    enablers: &[Enabler::DependencyPrefix("@babel/")],
    config: &[
        "babel.config.{json,js,cjs,mjs,cts,ts,mts}",
        ".babelrc.{json,js,cjs,mjs,cts}",
        ".babelrc",
    ],
    package_json_key: Some("babel"),
    binaries: &["babel"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Babel {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = dependencies(config.value, config.base, 0);
        for object in objects(config.value) {
            for extended in object.get("extends").map(strings).unwrap_or_default() {
                if is_relative(extended) {
                    out.push(PluginInput::ConfigFile(join_rel(config.base, extended)));
                }
            }
        }
        out
    }
}

/// The presets and plugins a Babel options object names, including those under `env` and `overrides`.
/// Also used by plugins that embed Babel options (`babel-loader`, `babel-jest`).
pub(crate) fn dependencies(options: &ConfigValue, base: &str, depth: u8) -> Vec<PluginInput> {
    let mut out = Vec::new();
    if depth > 8 {
        return out;
    }
    for object in objects(options) {
        for (key, kind) in [("presets", "preset"), ("plugins", "plugin")] {
            let Some(ConfigValue::Array(items)) = object.get(key) else {
                continue;
            };
            for name in items.iter().filter_map(name_of) {
                if is_relative(name) {
                    out.push(PluginInput::Entry(join_rel(base, name)));
                } else if let Some(package) = package_name(&resolve_name(name, kind)) {
                    out.push(PluginInput::Dependency(package));
                }
            }
        }
        if let Some(ConfigValue::Object(envs)) = object.get("env") {
            for (_, env) in envs {
                out.extend(dependencies(env, base, depth + 1));
            }
        }
        if let Some(overrides) = object.get("overrides") {
            out.extend(dependencies(overrides, base, depth + 1));
        }
    }
    out
}

/// Expands a shorthand like `env` to `@babel/preset-env` or `foo` to `babel-plugin-foo`.
fn resolve_name(identifier: &str, kind: &str) -> String {
    if let Some(module) = identifier.strip_prefix("module:") {
        return module.to_string();
    }
    let parts: Vec<&str> = identifier.split('/').collect();
    match parts.as_slice() {
        [scope, name, rest @ ..] if scope.starts_with('@') => {
            if !rest.is_empty() {
                identifier.to_string()
            } else if *scope == "@babel" {
                if name.starts_with(kind) {
                    identifier.to_string()
                } else {
                    format!("@babel/{kind}-{name}")
                }
            } else if name.contains(&format!("babel-{kind}")) {
                identifier.to_string()
            } else {
                format!("{scope}/babel-{kind}-{name}")
            }
        }
        [scope] if scope.starts_with('@') => format!("{scope}/babel-{kind}"),
        [name] if !name.starts_with(&format!("babel-{kind}")) => format!("babel-{kind}-{name}"),
        _ => identifier.to_string(),
    }
}
