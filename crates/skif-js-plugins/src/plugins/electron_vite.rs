//! electron-vite: the main, preload and renderer builds each have production entries; the config adds its own, and
//! `src/<name>/index` stays one, as in knip.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, html_entries, leaf_strings, production_entries};

/// The `electron-vite` plugin.
pub struct ElectronVite;

static SPEC: PluginSpec = PluginSpec {
    id: "electron-vite",
    enablers: &[Enabler::Dependency("electron-vite")],
    config: &["electron.vite.config.{js,mjs,cjs,ts,mts,cts}"],
    production: &[
        "src/main/index.{js,mjs,cjs,ts,mts,cts}",
        "src/preload/index.{js,mjs,cjs,ts,mts,cts}",
        "src/renderer/index.html",
    ],
    binaries: &["electron-vite"],
    ..PluginSpec::DEFAULT
};

const SECTIONS: [(&str, &str); 3] = [
    ("main", "src/main/index.{js,mjs,cjs,ts,mts,cts}"),
    ("preload", "src/preload/index.{js,mjs,cjs,ts,mts,cts}"),
    ("renderer", "src/renderer/index.html"),
];

impl Plugin for ElectronVite {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for (name, default) in SECTIONS {
            let mut declared = Vec::new();
            for path in [&["build", "rollupOptions", "input"][..], &["build", "lib", "entry"]] {
                if let Some(value) = config.value.get(name).and_then(|section| get_in(section, path)) {
                    declared.extend(leaf_strings(value));
                }
            }
            let patterns = if declared.is_empty() { vec![default] } else { declared };
            out.extend(section_entries(cx, config.base, name, &patterns));
        }
        out
    }

    fn resolve_without_config(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        SECTIONS
            .into_iter()
            .flat_map(|(name, default)| section_entries(cx, "", name, &[default]))
            .collect()
    }
}

fn section_entries(cx: &PluginContext<'_>, base: &str, name: &str, patterns: &[&str]) -> Vec<PluginInput> {
    let mut out = production_entries(base, patterns.iter().copied());
    if name == "renderer" {
        for pattern in patterns.iter().filter(|p| {
            std::path::Path::new(p)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
        }) {
            out.extend(html_entries(cx, base, pattern));
        }
    }
    out
}
