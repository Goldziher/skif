//! React Native: the community CLI packages and the autolinked dependencies of the config.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::package_name;

/// The `react-native` plugin.
pub struct ReactNative;

static SPEC: PluginSpec = PluginSpec {
    id: "react-native",
    enablers: &[Enabler::Dependency("react-native")],
    config: &["react-native.config.{js,cjs,mjs,ts}"],
    binaries: &["react-native"],
    ..PluginSpec::DEFAULT
};

const CLI_PACKAGES: &[&str] = &[
    "@react-native-community/cli",
    "@react-native-community/cli-platform-android",
    "@react-native-community/cli-platform-ios",
];

impl Plugin for ReactNative {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(ConfigValue::Object(dependencies)) = config.value.get("dependencies") {
            out.extend(
                dependencies
                    .iter()
                    .filter_map(|(name, _)| package_name(name))
                    .map(PluginInput::Dependency),
            );
        }
        if let Some(ConfigValue::Object(platforms)) = config.value.get("platforms") {
            out.extend(
                platforms
                    .iter()
                    .filter_map(|(_, platform)| platform.get("npmPackageName")?.as_str())
                    .filter_map(package_name)
                    .map(PluginInput::Dependency),
            );
        }
        out
    }

    fn resolve_without_config(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        CLI_PACKAGES
            .iter()
            .map(|package| PluginInput::OptionalDependency((*package).to_owned()))
            .collect()
    }
}
