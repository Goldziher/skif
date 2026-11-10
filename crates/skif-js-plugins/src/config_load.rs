//! Reads a config file of any supported format into a [`ConfigValue`]. JavaScript and TypeScript are
//! evaluated statically; nothing is executed.

use std::path::Path;

use skif_js_syntax::{ConfigValue, ModuleLoader, extract_config, extract_config_object};

/// Parses `source`, choosing the format from the file name: JavaScript and TypeScript are evaluated statically
/// (nothing is executed), JSON and JSONC, YAML and TOML are parsed. Values that cannot be known without running
/// code come back as [`ConfigValue::Unknown`]. `None` when the file is not understood.
#[must_use]
pub fn load_config(path: &Path, source: &str) -> Option<ConfigValue> {
    load_config_with(path, source, None)
}

/// Like [`load_config`], and a JavaScript or TypeScript config also evaluates the values it imports from the modules
/// `modules` follows (a base config of the project or of a workspace package).
#[must_use]
pub fn load_config_with(path: &Path, source: &str, modules: Option<&dyn ModuleLoader>) -> Option<ConfigValue> {
    load_config_and_call_arguments(path, source, modules).map(|(value, _)| value)
}

/// Like [`load_config_with`], with the arguments of the call a JavaScript or TypeScript config exports when that call
/// cannot be followed (see [`skif_js_syntax::ExtractedConfig::call_arguments`]); none for any other format.
pub(crate) fn load_config_and_call_arguments(
    path: &Path,
    source: &str,
    modules: Option<&dyn ModuleLoader>,
) -> Option<(ConfigValue, Vec<ConfigValue>)> {
    if path.parent().and_then(Path::file_name).is_some_and(|n| n == ".husky") {
        return Some((ConfigValue::String(source.to_string()), Vec::new()));
    }
    let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let value = match extension.as_deref() {
        Some("js" | "cjs" | "mjs" | "ts" | "cts" | "mts" | "jsx" | "tsx") => {
            return extract_config(source, path, modules).map(|config| (config.value, config.call_arguments));
        }
        Some("json" | "jsonc" | "json5") => json_like(source),
        Some("yml" | "yaml") => yaml(source),
        Some("toml") => toml(source),
        _ => json_like(source).or_else(|| yaml(source)),
    };
    value.map(|value| (value, Vec::new()))
}

/// Converts a parsed JSON document.
pub(crate) fn from_json(value: &serde_json::Value) -> ConfigValue {
    match value {
        serde_json::Value::Null => ConfigValue::Null,
        serde_json::Value::Bool(b) => ConfigValue::Bool(*b),
        serde_json::Value::Number(n) => n.as_f64().map_or(ConfigValue::Unknown, ConfigValue::Number),
        serde_json::Value::String(s) => ConfigValue::String(s.clone()),
        serde_json::Value::Array(items) => ConfigValue::Array(items.iter().map(from_json).collect()),
        serde_json::Value::Object(map) => {
            ConfigValue::Object(map.iter().map(|(k, v)| (k.clone(), from_json(v))).collect())
        }
    }
}

/// JSON with comments, trailing commas and single quotes is a JavaScript expression.
fn json_like(source: &str) -> Option<ConfigValue> {
    if source.trim().is_empty() {
        return None;
    }
    let wrapped = format!("module.exports = {source}");
    extract_config_object(&wrapped, Path::new("config.js"))
}

fn yaml(source: &str) -> Option<ConfigValue> {
    let value: serde_json::Value = yaml_serde::from_str(source).ok()?;
    Some(from_json(&value))
}

fn toml(source: &str) -> Option<ConfigValue> {
    let value: serde_json::Value = toml::from_str(source).ok()?;
    Some(from_json(&value))
}
