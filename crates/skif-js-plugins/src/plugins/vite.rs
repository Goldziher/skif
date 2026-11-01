//! Vite: HTML entry points, build inputs and pre-bundled dependencies.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, html_script_sources, join_rel, package_name, strings, values_of};

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
            if input.ends_with(".html") {
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
        out
    }
}

/// The scripts an HTML file loads, as production entries.
fn html_entries(cx: &PluginContext<'_>, base: &str, file: &str) -> Vec<PluginInput> {
    let html_path = join_rel(base, file);
    let Ok(html) = cx.fs.read_to_string(&cx.dir.join(&html_path)) else {
        return Vec::new();
    };
    let html_dir = html_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    html_script_sources(&html)
        .into_iter()
        .filter(|src| !src.contains("://") && !src.starts_with("//"))
        .map(|src| {
            let path = match src.strip_prefix('/') {
                Some(root_relative) => join_rel(base, root_relative),
                None => join_rel(html_dir, &src),
            };
            PluginInput::ProductionEntry(path)
        })
        .collect()
}
