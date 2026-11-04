//! SvelteKit: file-based routes, hooks and params, and the `$lib` alias.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::get_in;

/// The `sveltekit` plugin.
pub struct Sveltekit;

static SPEC: PluginSpec = PluginSpec {
    id: "sveltekit",
    enablers: &[Enabler::Dependency("@sveltejs/kit")],
    config: &["svelte.config.{js,cjs,mjs,ts}"],
    production: &[
        "src/routes/**/+{page,server,page.server,error,layout,layout.server}{,@*}.{js,ts,svelte}",
        "src/hooks{,.server,.client}.{js,ts}",
        "src/params/*.{js,ts}",
        "src/service-worker.{js,ts}",
        "src/service-worker/index.{js,ts}",
        "src/instrumentation.server.{js,ts}",
    ],
    binaries: &["svelte-kit"],
    ..PluginSpec::DEFAULT
};

const DEFAULT_LIB: &str = "src/lib";

impl Plugin for Sveltekit {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let lib = get_in(config.value, &["kit", "files", "lib"])
            .and_then(skif_js_syntax::ConfigValue::as_str)
            .unwrap_or(DEFAULT_LIB);
        vec![lib_alias(&config.join(lib))]
    }

    fn resolve_without_config(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vec![lib_alias(DEFAULT_LIB)]
    }
}

fn lib_alias(target: &str) -> PluginInput {
    PluginInput::Alias {
        find: "$lib".to_owned(),
        target: Some(target.to_owned()),
    }
}
