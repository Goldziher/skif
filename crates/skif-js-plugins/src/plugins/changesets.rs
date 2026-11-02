//! Changesets: the changelog generator named in `.changeset/config.json`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, name_of, package_name};

/// The `changesets` plugin.
pub struct Changesets;

static SPEC: PluginSpec = PluginSpec {
    id: "changesets",
    enablers: &[Enabler::Dependency("@changesets/cli")],
    root_only: true,
    config: &[".changeset/config.json"],
    binaries: &["changeset"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Changesets {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(changelog) = config.value.get("changelog").and_then(name_of) {
            out.extend(module_input(config.base, changelog, false));
        }
        if let Some(schema) = config.value.get("$schema").and_then(|s| s.as_str())
            && let Some((_, installed)) = schema.rsplit_once("node_modules/")
        {
            out.extend(package_name(installed).map(PluginInput::Dependency));
        }
        out
    }
}
