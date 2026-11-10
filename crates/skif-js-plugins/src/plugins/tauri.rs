//! Tauri: the commands Tauri runs before `tauri dev`, `tauri build` and bundling, from `tauri.conf.json` or
//! `Tauri.toml`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::script_in;

/// The `tauri` plugin.
pub struct Tauri;

static SPEC: PluginSpec = PluginSpec {
    id: "tauri",
    enablers: &[Enabler::Dependency("@tauri-apps/cli")],
    config: &[
        "src-tauri/tauri.conf.{json,json5}",
        "src-tauri/tauri.{macos,linux,windows,android,ios}.conf.{json,json5}",
        "src-tauri/Tauri.toml",
        "src-tauri/Tauri.{macos,linux,windows,android,ios}.toml",
        "tauri.conf.{json,json5}",
    ],
    binaries: &["tauri"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

const COMMANDS: [[&str; 2]; 3] = [
    ["beforeDevCommand", "before-dev-command"],
    ["beforeBuildCommand", "before-build-command"],
    ["beforeBundleCommand", "before-bundle-command"],
];

impl Plugin for Tauri {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// A command is a string, run in the workspace, or `{ script, cwd }`, run in `cwd`.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(build) = config.value.get("build") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for keys in COMMANDS {
            let Some(command) = keys.iter().find_map(|key| build.get(key)) else {
                continue;
            };
            let (script, dir) = match command {
                ConfigValue::String(script) => (script.as_str(), ""),
                other => (
                    other.get("script").and_then(ConfigValue::as_str).unwrap_or_default(),
                    other.get("cwd").and_then(ConfigValue::as_str).unwrap_or_default(),
                ),
            };
            if !script.trim().is_empty() {
                out.extend(script_in(dir, script.to_owned()));
            }
        }
        out
    }
}
