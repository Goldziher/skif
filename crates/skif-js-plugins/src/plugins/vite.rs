//! Vite: HTML entry points, build inputs and pre-bundled dependencies.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{alias_inputs, get_in, html_entries, join_rel, package_name, strings, values_of};

/// The `vite` plugin.
pub struct Vite;

static SPEC: PluginSpec = PluginSpec {
    id: "vite",
    enablers: &[
        Enabler::Dependency("vite"),
        Enabler::Dependency("vitest"),
        Enabler::Dependency("vite-plus"),
    ],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    binaries: &["vite"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Vite {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = html_entries(cx, config.base, "index.html");
        let inputs = get_in(config.value, &["build", "rollupOptions", "input"]);
        for input in inputs.into_iter().flat_map(values_of).flat_map(strings) {
            if std::path::Path::new(input)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
            {
                out.extend(html_entries(cx, config.base, input.trim_start_matches('/')));
            } else {
                out.push(PluginInput::ProductionEntry(config.join(input)));
            }
        }
        if let Some(entry) = get_in(config.value, &["build", "lib", "entry"]) {
            for path in values_of(entry).into_iter().flat_map(strings) {
                out.push(PluginInput::ProductionEntry(config.join(path)));
            }
        }
        if let Some(include) = get_in(config.value, &["optimizeDeps", "include"]) {
            for specifier in strings(include) {
                let id = specifier.split('>').next().unwrap_or_default().trim();
                out.extend(package_name(id).map(PluginInput::Dependency));
            }
        }
        if let Some(alias) = get_in(config.value, &["resolve", "alias"]) {
            out.extend(alias_inputs(alias, config));
        }
        out
    }

    fn resolve_without_config(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        html_entries(cx, "", "index.html")
    }
}
