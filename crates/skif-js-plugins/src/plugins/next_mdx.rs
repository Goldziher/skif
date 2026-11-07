//! Next.js MDX: the `mdx-components` module, and the remark, rehype and recma plugins named as strings in the options
//! of the `@next/mdx` call in `next.config.*`.

use skif_js_syntax::extract_imported_calls;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{name_of, package_name};

/// The `next-mdx` plugin.
pub struct NextMdx;

static SPEC: PluginSpec = PluginSpec {
    id: "next-mdx",
    enablers: &[Enabler::Dependency("@next/mdx")],
    config: &["next.config.{js,ts,cjs,mjs,mts}"],
    production: &["{,src/}mdx-components.{js,jsx,ts,tsx}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for NextMdx {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The config is read as source: what it exports is `withMDX(nextConfig)`, which is not evaluated.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for call in extract_imported_calls(config.source, config.path, &["@next/mdx"], &[]) {
            let Some(options) = call.arguments.first().and_then(|argument| argument.get("options")) else {
                continue;
            };
            for key in ["remarkPlugins", "rehypePlugins", "recmaPlugins"] {
                for plugin in options.get(key).and_then(|list| list.as_array()).unwrap_or_default() {
                    out.extend(name_of(plugin).and_then(package_name).map(PluginInput::Dependency));
                }
            }
        }
        out
    }
}
