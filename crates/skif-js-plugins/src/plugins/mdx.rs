//! MDX: the remark plugins the MDX language server loads, named in `mdx.plugins` of `tsconfig.json`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, name_of, package_name};

/// The `mdx` plugin.
pub struct Mdx;

static SPEC: PluginSpec = PluginSpec {
    id: "mdx",
    enablers: &[Enabler::Dependency("astro"), Enabler::Dependency("mdxlint")],
    config: &["tsconfig.json"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Mdx {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// Only the `mdx` key is read; the rest of `tsconfig.json` belongs to the `typescript` plugin.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        get_in(config.value, &["mdx", "plugins"])
            .and_then(|plugins| plugins.as_array())
            .unwrap_or_default()
            .iter()
            .filter_map(|plugin| name_of(plugin).and_then(package_name))
            .map(PluginInput::Dependency)
            .collect()
    }
}
