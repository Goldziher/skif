//! pnpm: the `.pnpmfile` hook file and the peer dependencies `packageExtensions` add.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `pnpm` plugin.
pub struct Pnpm;

static SPEC: PluginSpec = PluginSpec {
    id: "pnpm",
    enablers: &[Enabler::File("pnpm-lock.yaml"), Enabler::File("pnpm-workspace.yaml")],
    root_only: true,
    config: &["pnpm-workspace.yaml"],
    package_json_key: Some("pnpm"),
    entry: &[".pnpmfile.{cjs,mjs}"],
    binaries: &["pnpm", "pnpx"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Pnpm {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        peer_dependencies(config.value.get("packageExtensions"))
    }
}

/// The peer dependencies the entries of a `packageExtensions` object add, which need not be declared.
pub(crate) fn peer_dependencies(extensions: Option<&ConfigValue>) -> Vec<PluginInput> {
    let Some(ConfigValue::Object(extensions)) = extensions else {
        return Vec::new();
    };
    extensions
        .iter()
        .filter_map(|(_, extension)| extension.get("peerDependencies")?.as_object())
        .flatten()
        .map(|(name, _)| PluginInput::OptionalDependency(name.clone()))
        .collect()
}
