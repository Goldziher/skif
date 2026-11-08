//! Electron Forge: the makers, publishers and plugins of the forge config, and the entries and configs of its Vite
//! and webpack plugins, written as `{ name, config }` objects or as `new VitePlugin({ … })` and
//! `new WebpackPlugin({ … })` in `forge.config.*`.

use skif_js_syntax::{ConfigValue, extract_constructor_options};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::webpack::bundler_inputs;
use crate::support::{join_rel, objects, package_name, values_of};

/// The `electron-forge` plugin.
pub struct ElectronForge;

static SPEC: PluginSpec = PluginSpec {
    id: "electron-forge",
    enablers: &[Enabler::Dependency("@electron-forge/cli")],
    config: &["forge.config.{js,mjs,cjs,ts,mts,cts}"],
    package_json_key: Some("config.forge"),
    binaries: &["electron-forge"],
    ..PluginSpec::DEFAULT
};

impl Plugin for ElectronForge {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        // `"config": { "forge": "./forge.config.js" }` points at the config file.
        if let Some(path) = config.value.as_str() {
            return vec![PluginInput::ConfigFile(config.join(path))];
        }
        let mut out = Vec::new();
        for key in ["makers", "publishers", "plugins"] {
            for item in config.value.get(key).map(objects).unwrap_or_default() {
                let Some(name) = item.get("name").and_then(ConfigValue::as_str) else {
                    continue;
                };
                out.extend(package_name(name).map(PluginInput::Dependency));
                if let Some(options) = item.get("config") {
                    plugin_inputs(name, options, config, cx, &mut out);
                }
            }
        }
        if !config.source.is_empty() {
            for (constructor, name) in [("VitePlugin", "vite"), ("WebpackPlugin", "webpack")] {
                for key in ["build", "renderer", "mainConfig"] {
                    for value in extract_constructor_options(config.source, config.path, constructor, key) {
                        let options = ConfigValue::Object(vec![(key.to_owned(), value)]);
                        plugin_inputs(name, &options, config, cx, &mut out);
                    }
                }
            }
        }
        out
    }
}

/// What the options of the Vite or webpack plugin name: entry files (relative to the workspace, as Forge resolves
/// them) and config files of the bundler.
fn plugin_inputs(
    name: &str,
    options: &ConfigValue,
    config: &ConfigAst<'_>,
    cx: &PluginContext<'_>,
    out: &mut Vec<PluginInput>,
) {
    let entries = |value: Option<&ConfigValue>, out: &mut Vec<PluginInput>| {
        for item in value.map(values_of).unwrap_or_default() {
            if let Some(entry) = item.as_str() {
                out.push(PluginInput::ProductionEntry(join_rel("", entry)));
            }
        }
    };
    match name {
        "vite" | "@electron-forge/plugin-vite" => {
            for build in options.get("build").map(objects).unwrap_or_default() {
                entries(build.get("entry"), out);
                linked("vite", build.get("config"), config, cx, out);
            }
            for renderer in options.get("renderer").map(objects).unwrap_or_default() {
                linked("vite", renderer.get("config"), config, cx, out);
            }
        }
        "webpack" | "@electron-forge/plugin-webpack" => {
            linked("webpack", options.get("mainConfig"), config, cx, out);
            for renderer in options.get("renderer").map(objects).unwrap_or_default() {
                linked("webpack", renderer.get("config"), config, cx, out);
                for entry_point in renderer.get("entryPoints").map(objects).unwrap_or_default() {
                    entries(entry_point.get("js"), out);
                    if let Some(preload) = entry_point.get("preload") {
                        entries(preload.get("js"), out);
                        linked("webpack", preload.get("config"), config, cx, out);
                    }
                }
            }
        }
        _ => {}
    }
}

/// A bundler config named by path (read by that bundler's plugin) or written inline (a webpack object).
fn linked(
    plugin: &'static str,
    value: Option<&ConfigValue>,
    config: &ConfigAst<'_>,
    cx: &PluginContext<'_>,
    out: &mut Vec<PluginInput>,
) {
    match value {
        Some(ConfigValue::String(path)) => out.push(PluginInput::LinkedConfig {
            plugin,
            path: config.join(path),
        }),
        Some(object @ ConfigValue::Object(_)) if plugin == "webpack" => {
            out.extend(bundler_inputs(
                &ConfigAst {
                    value: object,
                    ..*config
                },
                cx,
            ));
        }
        _ => {}
    }
}
