//! tsdown: the `entry` of each build is a production entry; packages in `deps.neverBundle` stay external.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::tsup::entry_inputs;
use crate::support::{get_in, objects, strings};

/// The `tsdown` plugin.
pub struct Tsdown;

static SPEC: PluginSpec = PluginSpec {
    id: "tsdown",
    enablers: &[Enabler::Dependency("tsdown")],
    config: &["tsdown.config.{ts,mts,cts,js,mjs,cjs,json}"],
    package_json_key: Some("tsdown"),
    binaries: &["tsdown"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Tsdown {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = entry_inputs(config, &["entry"]);
        for build in objects(config.value) {
            if let Some(never_bundle) = get_in(build, &["deps", "neverBundle"]) {
                out.extend(
                    strings(never_bundle)
                        .into_iter()
                        .map(|name| PluginInput::OptionalDependency(name.to_owned())),
                );
            }
        }
        out
    }
}
