//! Capacitor: the plugins `includePlugins` lists, and the Android and iOS platform packages when their native projects
//! exist.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::strings;

/// The `capacitor` plugin.
pub struct Capacitor;

static SPEC: PluginSpec = PluginSpec {
    id: "capacitor",
    enablers: &[Enabler::DependencyPrefix("@capacitor/")],
    config: &["capacitor.config.{json,js,ts}"],
    binaries: &["cap"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Capacitor {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out: Vec<PluginInput> = config
            .value
            .get("includePlugins")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .map(|name| PluginInput::Dependency(name.to_owned()))
            .collect();
        let exists = |path: &str| cx.fs.is_file(&cx.dir.join(config.join(path)));
        if exists("android/capacitor.settings.gradle") {
            out.push(PluginInput::Dependency("@capacitor/android".to_owned()));
        }
        if exists("ios/App/Podfile") || exists("ios/App/CapApp-SPM/Package.swift") {
            out.push(PluginInput::Dependency("@capacitor/ios".to_owned()));
        }
        out
    }
}
