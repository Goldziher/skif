//! simple-git-hooks: every hook is a script.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::script_inputs;

/// The `simple-git-hooks` plugin.
pub struct SimpleGitHooks;

static SPEC: PluginSpec = PluginSpec {
    id: "simple-git-hooks",
    enablers: &[Enabler::Dependency("simple-git-hooks")],
    config: &[
        ".simple-git-hooks.{js,cjs,mjs,json}",
        "simple-git-hooks.{js,cjs,mjs,json}",
    ],
    package_json_key: Some("simple-git-hooks"),
    binaries: &["simple-git-hooks"],
    ..PluginSpec::DEFAULT
};

impl Plugin for SimpleGitHooks {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let ConfigValue::Object(hooks) = config.value else {
            return Vec::new();
        };
        let mut out = vec![PluginInput::Dependency("simple-git-hooks".to_owned())];
        out.extend(
            hooks
                .iter()
                .filter(|(name, _)| name != "preserveUnused")
                .flat_map(|(_, hook)| script_inputs(hook)),
        );
        out
    }
}
