//! Parcel: the config packages, plugins and local plugin files named in `.parcelrc`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, module_input, strings, values_of};

/// The `parcel` plugin.
pub struct Parcel;

static SPEC: PluginSpec = PluginSpec {
    id: "parcel",
    enablers: &[Enabler::Dependency("parcel"), Enabler::Dependency("@parcel/core")],
    config: &[".parcelrc"],
    binaries: &["parcel"],
    ..PluginSpec::DEFAULT
};

/// Keys that hold a plugin name or a list of them.
const LISTS: &[&str] = &["resolvers", "bundler", "namers", "reporters"];

/// Keys that map file globs to a plugin name or a list of them.
const MAPS: &[&str] = &[
    "transformers",
    "runtimes",
    "packagers",
    "optimizers",
    "compressors",
    "validators",
];

impl Plugin for Parcel {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let mut out: Vec<PluginInput> = value
            .get("extends")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|name| config_ref(config.base, name))
            .collect();
        let mut names: Vec<&str> = LISTS
            .iter()
            .filter_map(|key| value.get(key))
            .flat_map(strings)
            .collect();
        for key in MAPS {
            if let Some(map) = value.get(key).filter(|map| matches!(map, ConfigValue::Object(_))) {
                names.extend(values_of(map).into_iter().flat_map(strings));
            }
        }
        // `...` stands for the plugins of the extended config.
        out.extend(
            names
                .into_iter()
                .filter(|name| *name != "...")
                .filter_map(|name| module_input(config.base, name, false)),
        );
        out
    }
}
