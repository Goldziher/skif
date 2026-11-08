//! Ladle: stories and `.ladle/components.*` are entries; `stories` and `viteConfig` of `.ladle/config.*` are read,
//! the Vite config by the `vitest` plugin.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, strings};

/// The `ladle` plugin.
pub struct Ladle;

static SPEC: PluginSpec = PluginSpec {
    id: "ladle",
    enablers: &[Enabler::Dependency("@ladle/react")],
    config: &[".ladle/config.{mjs,js,ts}"],
    entry: &[
        ".ladle/components.{js,jsx,ts,tsx}",
        "src/**/*.stories.{js,jsx,ts,tsx,mdx}",
    ],
    project: &[".ladle/**/*.{js,jsx,ts,tsx}"],
    binaries: &["ladle"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Ladle {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out: Vec<PluginInput> = config
            .value
            .get("stories")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|pattern| entry_glob("", pattern))
            .map(PluginInput::Entry)
            .collect();
        if let Some(vite) = config.value.get("viteConfig").and_then(|value| value.as_str())
            && let Some(path) = entry_glob("", vite)
        {
            out.push(PluginInput::LinkedConfig { plugin: "vitest", path });
        }
        out
    }
}
