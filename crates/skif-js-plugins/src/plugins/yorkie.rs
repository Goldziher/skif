//! yorkie: the commands of the git hooks in `package.json#gitHooks` are scripts.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{leaf_strings, values_of};

/// The `yorkie` plugin.
pub struct Yorkie;

static SPEC: PluginSpec = PluginSpec {
    id: "yorkie",
    enablers: &[Enabler::Dependency("yorkie")],
    package_json_key: Some("gitHooks"),
    ..PluginSpec::DEFAULT
};

impl Plugin for Yorkie {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = vec![PluginInput::Dependency("yorkie".to_owned())];
        if config.value.as_object().is_some() {
            out.extend(
                values_of(config.value)
                    .into_iter()
                    .flat_map(leaf_strings)
                    .map(|script| PluginInput::Script(script.to_owned())),
            );
        }
        out
    }
}
