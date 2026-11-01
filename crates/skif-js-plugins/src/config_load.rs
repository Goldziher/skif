//! Reads a config file of any supported format into a [`ConfigValue`]. JavaScript and TypeScript are
//! evaluated statically; nothing is executed.

use std::path::Path;

use skif_js_syntax::{ConfigValue, extract_config_object};

/// Parses `source`, choosing the format from the file name. `None` when the file is not understood.
pub(crate) fn load_config(path: &Path, source: &str) -> Option<ConfigValue> {
    if path.parent().and_then(Path::file_name).is_some_and(|n| n == ".husky") {
        return Some(ConfigValue::String(source.to_string()));
    }
    let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("js" | "cjs" | "mjs" | "ts" | "cts" | "mts" | "jsx" | "tsx") => extract_config_object(source, path),
        Some("json" | "jsonc" | "json5") => json_like(source),
        Some("yml" | "yaml") => yaml(source),
        Some("toml") => toml(source),
        _ => json_like(source).or_else(|| yaml(source)),
    }
}

/// Converts a parsed JSON document.
pub(crate) fn from_json(value: &serde_json::Value) -> ConfigValue {
    match value {
        serde_json::Value::Null => ConfigValue::Null,
        serde_json::Value::Bool(b) => ConfigValue::Bool(*b),
        serde_json::Value::Number(n) => n.as_f64().map_or(ConfigValue::Unknown, ConfigValue::Number),
        serde_json::Value::String(s) => ConfigValue::String(s.clone()),
        serde_json::Value::Array(items) => ConfigValue::Array(items.iter().map(from_json).collect()),
        serde_json::Value::Object(map) => ConfigValue::Object(map.iter().map(|(k, v)| (k.clone(), from_json(v))).collect()),
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
