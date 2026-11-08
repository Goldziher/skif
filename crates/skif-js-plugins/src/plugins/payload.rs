//! Payload CMS: the admin import map, at the path `buildConfig({ admin: { importMap: { importMapFile } } })` names or
//! under the admin route of the Next.js app, is an entry; migrations and the generated types are not reported.

use skif_js_syntax::{ConfigValue, extract_imported_calls};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, module_input};

/// The `payload` plugin.
pub struct Payload;

static SPEC: PluginSpec = PluginSpec {
    id: "payload",
    enablers: &[Enabler::Dependency("payload")],
    config: &["payload.config.ts", "src/payload.config.ts"],
    binaries: &["payload"],
    ignore: &[
        "migrations/**",
        "src/migrations/**",
        "payload-types.ts",
        "src/payload-types.ts",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for Payload {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the `buildConfig` call in the source, not from what the config exports.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let calls = extract_imported_calls(config.source, config.path, &["payload"], &[]);
        let options = calls
            .iter()
            .find(|call| call.export == "buildConfig")
            .and_then(|call| call.arguments.first());
        if let Some(file) = options
            .and_then(|options| get_in(options, &["admin", "importMap", "importMapFile"]))
            .and_then(ConfigValue::as_str)
        {
            return module_input(config.base, file, true).into_iter().collect();
        }
        let route = options
            .and_then(|options| get_in(options, &["routes", "admin"]))
            .and_then(ConfigValue::as_str)
            .unwrap_or("/admin");
        ["app", "src/app"]
            .into_iter()
            .map(|app| PluginInput::Entry(format!("{app}/(payload){route}/importMap.js")))
            .collect()
    }
}
