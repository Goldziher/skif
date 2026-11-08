//! mdxlint: the remark plugins of `.mdxlintrc*` or `package.json#mdxlint`; a bare name `x` is `remark-x`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, module_input, name_of};

/// The `mdxlint` plugin.
pub struct Mdxlint;

static SPEC: PluginSpec = PluginSpec {
    id: "mdxlint",
    enablers: &[Enabler::Dependency("mdxlint")],
    config: &[".mdxlintrc", ".mdxlintrc.{json,js,cjs,mjs,yml,yaml}"],
    package_json_key: Some("mdxlint"),
    binaries: &["mdxlint"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Mdxlint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(plugins) = config.value.get("plugins").and_then(|value| value.as_array()) else {
            return Vec::new();
        };
        plugins
            .iter()
            .filter_map(name_of)
            .filter_map(|name| {
                let name = if is_relative(name)
                    || name.starts_with(['/', '@'])
                    || name.starts_with("remark-")
                    || name.starts_with("mdxlint-")
                {
                    name.to_owned()
                } else {
                    format!("remark-{name}")
                };
                module_input(config.base, &name, false)
            })
            .collect()
    }
}
