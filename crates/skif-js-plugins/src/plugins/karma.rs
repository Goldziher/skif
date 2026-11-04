//! Karma: the frameworks and plugins of the config are packages, `files` are entries.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{conventional, entry_glob, join_rel, strings};

/// The `karma` plugin.
pub struct Karma;

static SPEC: PluginSpec = PluginSpec {
    id: "karma",
    enablers: &[Enabler::Dependency("karma")],
    config: &[
        "{,config/}karma.{conf,config}.{js,cjs,mjs,ts}",
        "{,config/}karma.{conf,config}.*.{js,cjs,mjs,ts}",
    ],
    binaries: &["karma"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Karma {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(frameworks) = config.value.get("frameworks") {
            out.extend(
                strings(frameworks)
                    .into_iter()
                    .map(|name| PluginInput::Dependency(conventional(name, "karma"))),
            );
        }
        if let Some(plugins) = config.value.get("plugins") {
            out.extend(
                strings(plugins)
                    .into_iter()
                    .filter(|name| !name.starts_with("framework:") && !name.starts_with("inline:"))
                    .filter_map(crate::support::package_name)
                    .map(PluginInput::Dependency),
            );
        }
        let base_path = config
            .value
            .get("basePath")
            .and_then(ConfigValue::as_str)
            .map_or_else(|| config.base.to_owned(), |path| join_rel(config.base, path));
        if let Some(ConfigValue::Array(files)) = config.value.get("files") {
            out.extend(
                files
                    .iter()
                    .filter_map(|file| match file {
                        ConfigValue::String(pattern) => Some(pattern.as_str()),
                        other => other.get("pattern").and_then(ConfigValue::as_str),
                    })
                    .filter(|pattern| !pattern.contains("://"))
                    .filter_map(|pattern| entry_glob(&base_path, pattern))
                    .map(PluginInput::Entry),
            );
        }
        out
    }
}
