//! Astro: pages, content collections, middleware and actions.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `astro` plugin.
pub struct Astro;

static SPEC: PluginSpec = PluginSpec {
    id: "astro",
    enablers: &[Enabler::Dependency("astro")],
    config: &["astro.config.{js,cjs,mjs,ts,mts}"],
    entry: &["src/content/config.ts", "src/content.config.ts"],
    production: &[
        "src/pages/**/*.{astro,mdx,js,ts}",
        "!src/pages/**/_*",
        "!src/pages/**/_*/**",
        "src/content/**/*.mdx",
        "src/middleware.{js,ts}",
        "src/middleware/index.{js,ts}",
        "src/actions/index.{js,ts}",
    ],
    binaries: &["astro"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Astro {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// `astro check` needs `@astrojs/check`, which is never imported.
    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let checks = cx.scripts.values().any(|script| {
            let words: Vec<&str> = script.split_whitespace().collect();
            words.windows(2).any(|pair| pair == ["astro", "check"])
        });
        if checks {
            vec![PluginInput::Dependency("@astrojs/check".to_owned())]
        } else {
            Vec::new()
        }
    }
}
