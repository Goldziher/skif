//! textlint: the rules, filter rules and plugins of `.textlintrc*` or the `textlint` key of `package.json`, and of
//! `--rule`, `--preset` and `--plugin` in scripts, named by textlint's package conventions; a declared package that
//! matches the name is preferred.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::flag_values;
use crate::support::{is_relative, join_rel, strings};

/// The `textlint` plugin.
pub struct Textlint;

static SPEC: PluginSpec = PluginSpec {
    id: "textlint",
    enablers: &[Enabler::Dependency("textlint")],
    config: &[".textlintrc", ".textlintrc.{json,yaml,yml,js,cjs}"],
    package_json_key: Some("textlint"),
    binaries: &["textlint"],
    ..PluginSpec::DEFAULT
};

const RULE_PREFIX: &str = "textlint-rule-";
const PRESET_PREFIX: &str = "textlint-rule-preset-";
const FILTER_RULE_PREFIX: &str = "textlint-filter-rule-";
const PLUGIN_PREFIX: &str = "textlint-plugin-";

impl Plugin for Textlint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(ConfigValue::Object(rules)) = config.value.get("rules") {
            for (key, options) in rules {
                if !matches!(options, ConfigValue::Bool(false) | ConfigValue::Null) {
                    out.push(rule(key, config.base, cx));
                }
            }
        }
        if let Some(ConfigValue::Object(filters)) = config.value.get("filters") {
            for (key, _) in filters {
                out.push(module(FILTER_RULE_PREFIX, key, config.base, cx));
            }
        }
        let plugins: Vec<&str> = match config.value.get("plugins") {
            Some(ConfigValue::Object(plugins)) => plugins.iter().map(|(key, _)| key.as_str()).collect(),
            Some(other) => strings(other),
            None => Vec::new(),
        };
        out.extend(
            plugins
                .into_iter()
                .map(|key| module(PLUGIN_PREFIX, key, config.base, cx)),
        );
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for script in cx.scripts.values() {
            let list = |flag: &str| -> Vec<String> {
                flag_values(script, SPEC.binaries, &[flag])
                    .iter()
                    .flat_map(|value| value.split(','))
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            for key in list("--rule") {
                out.push(rule(&key, "", cx));
            }
            for preset in list("--preset") {
                let key = if is_relative(&preset) || preset.contains("preset-") {
                    preset
                } else {
                    format!("preset-{preset}")
                };
                out.push(rule(&key, "", cx));
            }
            for key in list("--plugin") {
                out.push(module(PLUGIN_PREFIX, &key, "", cx));
            }
        }
        out
    }
}

/// A rule or rule preset; a relative path is a local rule file.
fn rule(key: &str, base: &str, cx: &PluginContext<'_>) -> PluginInput {
    if is_relative(key) || key.starts_with('/') {
        return PluginInput::Entry(join_rel(base, key));
    }
    if !is_preset(key) {
        return module(RULE_PREFIX, key, base, cx);
    }
    let name = key.strip_prefix("preset-").unwrap_or(key);
    let name = match name.strip_prefix('@').and_then(|rest| rest.split_once("/preset-")) {
        Some((scope, rest)) => format!("@{scope}/{rest}"),
        None => name.to_owned(),
    };
    let candidates = [
        format!("{PRESET_PREFIX}{name}"),
        name.clone(),
        full_name(PRESET_PREFIX, &name),
        key.to_owned(),
    ];
    package(PRESET_PREFIX, &name, &candidates, cx)
}

/// A filter rule or a plugin; a relative path is a local file.
fn module(prefix: &str, key: &str, base: &str, cx: &PluginContext<'_>) -> PluginInput {
    if is_relative(key) || key.starts_with('/') {
        return PluginInput::Entry(join_rel(base, key));
    }
    package(prefix, key, &[full_name(prefix, key), key.to_owned()], cx)
}

fn is_preset(key: &str) -> bool {
    key.starts_with("preset-")
        || key.starts_with(PRESET_PREFIX)
        || (key.starts_with('@') && (key.contains("/preset-") || key.contains(&format!("/{PRESET_PREFIX}"))))
}

/// The first candidate the workspace declares, or else the conventional name.
fn package(prefix: &str, name: &str, candidates: &[String], cx: &PluginContext<'_>) -> PluginInput {
    let declared = candidates
        .iter()
        .find(|candidate| cx.has_dependency_in_ancestors(candidate));
    let name = declared.cloned().unwrap_or_else(|| {
        if has_prefix(prefix, name) {
            name.to_owned()
        } else {
            full_name(prefix, name)
        }
    });
    PluginInput::Dependency(name)
}

fn has_prefix(prefix: &str, name: &str) -> bool {
    let bare = if name.starts_with('@') {
        name.split_once('/').map_or(name, |(_, rest)| rest)
    } else {
        name
    };
    bare.starts_with(prefix)
}

/// `foo` becomes `<prefix>foo` and `@scope/foo` becomes `@scope/<prefix>foo`; a scoped name that already carries the
/// prefix, or is the bare stem (`@scope/textlint-rule`), is kept.
fn full_name(prefix: &str, name: &str) -> String {
    if let Some((scope, rest)) = name.strip_prefix('@').and_then(|rest| rest.split_once('/')) {
        if rest == prefix.trim_end_matches('-') || rest.starts_with(prefix) {
            return name.to_owned();
        }
        return format!("@{scope}/{prefix}{rest}");
    }
    format!("{prefix}{name}")
}
