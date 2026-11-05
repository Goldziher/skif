//! remark: plugins are packages named `remark-<name>` or by their full name.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{conventional, module_input, name_of, values_of};

/// The `remark` plugin.
pub struct Remark;

static SPEC: PluginSpec = PluginSpec {
    id: "remark",
    enablers: &[Enabler::Dependency("remark-cli")],
    config: &[
        ".remarkrc",
        ".remarkrc.json",
        ".remarkrc.{js,cjs,mjs}",
        ".remarkrc.{yml,yaml}",
    ],
    package_json_key: Some("remarkConfig"),
    binaries: &["remark"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Remark {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(plugins) = config.value.get("plugins") else {
            return Vec::new();
        };
        values_of(plugins)
            .into_iter()
            .filter_map(name_of)
            .filter_map(|name| {
                if name.starts_with('.') {
                    return module_input(config.base, name, false);
                }
                let prefixed = conventional(name, "remark");
                let chosen = if cx.has_dependency(&prefixed) || !cx.has_dependency(name) {
                    prefixed
                } else {
                    name.to_owned()
                };
                Some(PluginInput::Dependency(chosen))
            })
            .collect()
    }
}
