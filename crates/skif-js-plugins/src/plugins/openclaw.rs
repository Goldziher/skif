//! `OpenClaw`: the extension, setup, provider catalog, channel state and hook modules and the static asset sources
//! that `package.json#openclaw` names are production entries, and its asset scripts are scripts; the
//! `providerCatalogEntry` of `openclaw.plugin.json` is a production entry too.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, get_in, objects, property_values, strings};

/// The `openclaw` plugin.
pub struct Openclaw;

static SPEC: PluginSpec = PluginSpec {
    id: "openclaw",
    enablers: &[Enabler::ManifestKey("openclaw")],
    config: &["openclaw.plugin.json"],
    package_json_key: Some("openclaw"),
    ..PluginSpec::DEFAULT
};

const ENTRY_PATHS: &[&[&str]] = &[
    &["extensions"],
    &["runtimeExtensions"],
    &["setupEntry"],
    &["runtimeSetupEntry"],
    &["providerCatalogEntry"],
    &["channel", "configuredState", "specifier"],
    &["channel", "persistedAuthState", "specifier"],
];

fn entries<'a>(paths: impl IntoIterator<Item = &'a str>) -> impl Iterator<Item = PluginInput> {
    paths
        .into_iter()
        .filter_map(|path| entry_glob("", path))
        .map(PluginInput::ProductionEntry)
}

impl Plugin for Openclaw {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        if config.file_name() == "openclaw.plugin.json" {
            return entries(property_values(value, "providerCatalogEntry")).collect();
        }
        let mut out: Vec<PluginInput> = Vec::new();
        for path in ENTRY_PATHS {
            out.extend(entries(get_in(value, path).map_or_default(strings)));
        }
        for hook in value.get("hooks").map_or_default(strings) {
            let dir = hook.trim().trim_end_matches('/');
            if !dir.is_empty() {
                out.extend(entries([format!("{dir}/{{handler,index}}.{{ts,js}}").as_str()]));
            }
        }
        let assets = get_in(value, &["build", "staticAssets"]).map_or_default(objects);
        out.extend(entries(
            assets
                .into_iter()
                .filter_map(|asset| asset.get("source").and_then(ConfigValue::as_str)),
        ));
        for key in ["build", "copy"] {
            if let Some(script) = get_in(value, &["assetScripts", key]).and_then(ConfigValue::as_str) {
                out.push(PluginInput::Script(script.to_owned()));
            }
        }
        out
    }
}
