//! PM2: the `script` of each application in the ecosystem file is an entry.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, join_rel, looks_like_file};

/// The `pm2` plugin.
pub struct Pm2;

static SPEC: PluginSpec = PluginSpec {
    id: "pm2",
    enablers: &[Enabler::Dependency("pm2")],
    config: &["pm2.config.{json,js,cjs,mjs}", "ecosystem.config.{json,js,cjs,mjs}"],
    binaries: &["pm2", "pm2-runtime"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Pm2 {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut applications: Vec<&ConfigValue> = Vec::new();
        match config.value {
            ConfigValue::Array(items) => applications.extend(items),
            other => match other.get("apps") {
                Some(ConfigValue::Array(items)) => applications.extend(items),
                _ => applications.push(other),
            },
        }
        applications
            .into_iter()
            .filter_map(|application| application.get("script")?.as_str())
            .filter(|script| is_relative(script) || looks_like_file(script))
            .map(|script| PluginInput::Entry(join_rel(config.base, script)))
            .collect()
    }
}
