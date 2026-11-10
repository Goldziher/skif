//! `ESLint`: flat and legacy configs. Plugins, configs and parsers named by string follow `ESLint`'s
//! package naming conventions.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{conventional, get_in, is_relative, module_input, objects, strings};

/// The `eslint` plugin.
pub struct Eslint;

static SPEC: PluginSpec = PluginSpec {
    id: "eslint",
    enablers: &[
        Enabler::Dependency("eslint"),
        Enabler::Dependency("@eslint/js"),
        Enabler::ManifestNamePrefix("eslint-config"),
        Enabler::ManifestNameContains("/eslint-config"),
    ],
    config: &[
        "eslint.config.{js,cjs,mjs,ts,cts,mts}",
        ".eslintrc",
        ".eslintrc.{js,json,cjs,yml,yaml}",
    ],
    package_json_key: Some("eslintConfig"),
    binaries: &["eslint"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Eslint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        collect(config.value, config, &mut out, 0);
        out
    }
}

/// What a config object, or an array of them, names: shared configs, plugins, parsers and resolvers; `xo` reuses it.
pub(crate) fn collect(value: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 8 {
        return;
    }
    // A string `extends` of a flat config names a config of a plugin the config imports (`js/recommended`), not a
    // package; knip reads only the settings of a flat config.
    let flat = config.file_name().starts_with("eslint.config.");
    for object in objects(value) {
        if let Some(extends) = object.get("extends").filter(|_| !flat) {
            for specifier in strings(extends) {
                out.extend(extends_input(config, specifier));
            }
        }
        if let Some(ConfigValue::Array(plugins)) = object.get("plugins") {
            for name in plugins.iter().filter_map(ConfigValue::as_str) {
                out.push(PluginInput::Dependency(conventional(name, "eslint-plugin")));
            }
        }
        let parser_paths: [&[&str]; 3] = [
            &["parser"],
            &["parserOptions", "parser"],
            &["languageOptions", "parser"],
        ];
        for path in parser_paths {
            if let Some(parser) = get_in(object, path).and_then(ConfigValue::as_str)
                && parser != "espree"
            {
                out.extend(module_input(config.base, parser, false));
            }
        }
        if let Some(settings) = object.get("settings") {
            resolvers(settings, out);
        }
        if let Some(overrides) = object.get("overrides") {
            collect(overrides, config, out, depth + 1);
        }
    }
}

fn extends_input(config: &ConfigAst<'_>, specifier: &str) -> Option<PluginInput> {
    if specifier.starts_with("eslint:") {
        // A built-in config (`eslint:recommended`) ships with ESLint itself, as knip credits it.
        return Some(PluginInput::OptionalDependency("eslint".to_owned()));
    }
    if let Some(rest) = specifier.strip_prefix("plugin:") {
        let parts: Vec<&str> = rest.split('/').collect();
        let plugin = match parts.as_slice() {
            [scope, name, _, ..] if scope.starts_with('@') => format!("{scope}/{name}"),
            [first, ..] => (*first).to_string(),
            [] => return None,
        };
        return Some(PluginInput::Dependency(conventional(&plugin, "eslint-plugin")));
    }
    if is_relative(specifier) {
        return Some(PluginInput::ConfigFile(config.join(specifier)));
    }
    Some(PluginInput::Dependency(conventional(specifier, "eslint-config")))
}

/// `settings["import/resolver"]` names resolver packages; `settings["import/parsers"]` names parsers.
fn resolvers(settings: &ConfigValue, out: &mut Vec<PluginInput>) {
    if let Some(resolver) = settings.get("import/resolver") {
        let names: Vec<&str> = match resolver {
            ConfigValue::Object(entries) => entries.iter().map(|(k, _)| k.as_str()).collect(),
            other => strings(other),
        };
        // `eslint-plugin-import` falls back to its own resolution without the resolver package; knip reads it as an
        // optional reference.
        for name in names {
            out.push(PluginInput::OptionalDependency(conventional(
                name,
                "eslint-import-resolver",
            )));
        }
    }
    if let Some(ConfigValue::Object(parsers)) = settings.get("import/parsers") {
        for (name, _) in parsers {
            out.extend(module_input("", name, false));
        }
    }
}
