//! Nitro: the server entry and the api, routes, middleware and plugins directories are production entries; modules
//! are packages and `#nitro-internal-` imports are virtual.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, name_of, package_name, production_entries, values_of};

/// The `nitro` plugin.
pub struct Nitro;

static SPEC: PluginSpec = PluginSpec {
    id: "nitro",
    enablers: &[Enabler::Dependency("nitropack"), Enabler::Dependency("nitro")],
    config: &["nitro.config.{js,cjs,mjs,ts,cts,mts}"],
    production: &[
        "server.{js,mjs,ts}",
        "api/**/*.ts",
        "routes/**/*.ts",
        "middleware/**/*.ts",
        "plugins/**/*.ts",
        ".nitro/types/*.d.ts",
    ],
    binaries: &["nitro", "nitropack"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Nitro {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let text = |key: &str, default: &'static str| -> String {
            config
                .value
                .get(key)
                .and_then(ConfigValue::as_str)
                .map_or_else(|| default.to_owned(), str::to_owned)
        };
        let source_dir = text("srcDir", ".");
        let patterns = [
            text("serverEntry", "server.{js,mjs,ts}"),
            join_rel(&text("apiDir", "api"), "**/*.ts"),
            join_rel(&text("routesDir", "routes"), "**/*.ts"),
            "middleware/**/*.ts".to_owned(),
            "plugins/**/*.ts".to_owned(),
        ];
        let rooted: Vec<String> = patterns.iter().map(|pattern| join_rel(&source_dir, pattern)).collect();
        let mut out = production_entries(config.base, rooted.iter().map(String::as_str));
        if let Some(modules) = config.value.get("modules") {
            out.extend(
                values_of(modules)
                    .into_iter()
                    .filter_map(name_of)
                    .filter_map(package_name)
                    .map(PluginInput::Dependency),
            );
        }
        out.push(PluginInput::VirtualModule("#nitro-internal-*".to_owned()));
        out
    }
}
