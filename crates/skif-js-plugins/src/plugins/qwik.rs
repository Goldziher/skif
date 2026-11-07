//! Qwik: the app's entry files and Qwik City routes under the `srcDir` of `qwikVite({ … })` and the `routesDir` of
//! `qwikCity({ … })` in `vite.config.*`, and the virtual modules the Qwik build provides.

use skif_js_syntax::extract_imported_calls;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::join_rel;

/// The `qwik` plugin.
pub struct Qwik;

const ENTRY: &str = "src/entry.dev.tsx";
const PRODUCTION: &[&str] = &["src/root.tsx", "src/entry.*.tsx"];
const ROUTES: &str = "**/*.{tsx,ts,md,mdx}";

static SPEC: PluginSpec = PluginSpec {
    id: "qwik",
    enablers: &[Enabler::Dependency("@builder.io/qwik")],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    entry: &[ENTRY],
    production: &["src/root.tsx", "src/entry.*.tsx", "src/routes/**/*.{tsx,ts,md,mdx}"],
    binaries: &["qwik"],
    ..PluginSpec::DEFAULT
};

const VIRTUAL_MODULES: &[&str] = &["@qwik-city-plan", "@qwik-city-sw-register", "@qwik-client-manifest"];

impl Plugin for Qwik {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the plugin calls in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let calls = extract_imported_calls(
            config.source,
            config.path,
            &["@builder.io/qwik/optimizer", "@builder.io/qwik-city/vite"],
            &[],
        );
        let option = |export: &str, key: &str| -> Vec<String> {
            calls
                .iter()
                .filter(|call| call.export == export)
                .filter_map(|call| call.arguments.first()?.get(key)?.as_str().map(str::to_owned))
                .collect()
        };
        let src_dir = option("qwikVite", "srcDir")
            .into_iter()
            .next()
            .unwrap_or_else(|| "src".to_owned());
        let mut routes_dirs = option("qwikCity", "routesDir");
        if routes_dirs.is_empty() {
            routes_dirs.push(format!("{src_dir}/routes"));
        }
        let in_src = |pattern: &str| config.join(&join_rel(&src_dir, pattern.trim_start_matches("src/")));
        let mut out = vec![PluginInput::Entry(in_src(ENTRY))];
        out.extend(
            PRODUCTION
                .iter()
                .map(|pattern| PluginInput::ProductionEntry(in_src(pattern))),
        );
        out.extend(
            routes_dirs
                .iter()
                .map(|dir| PluginInput::ProductionEntry(config.join(&join_rel(dir, ROUTES)))),
        );
        out
    }

    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        VIRTUAL_MODULES
            .iter()
            .map(|name| PluginInput::VirtualModule((*name).to_owned()))
            .collect()
    }
}
