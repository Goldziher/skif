//! Next.js: the file-system router entry points and a custom `pageExtensions`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::strings;

/// The `next` plugin.
pub struct Next;

static SPEC: PluginSpec = PluginSpec {
    id: "next",
    enablers: &[Enabler::Dependency("next")],
    config: &["next.config.{js,ts,cjs,mjs,mts}"],
    entry: &[
        "{,src/}app/{,[(]*[)]/}{manifest,robots}.{js,ts}",
        "{,src/}app/**/sitemap.{js,ts}",
        "{,src/}app/**/{icon,apple-icon,opengraph-image,twitter-image}.{js,jsx,ts,tsx}",
        "{,src/}{instrumentation,instrumentation-client,middleware,proxy}.{js,jsx,ts,tsx}",
        "{,src/}app/global-{error,not-found}.{js,jsx,ts,tsx}",
        "{,src/}app/**/{default,error,forbidden,loading,not-found,unauthorized}.{js,jsx,ts,tsx}",
        "{,src/}app/**/{layout,page,route,template}.{js,jsx,ts,tsx}",
        "{,src/}pages/**/*.{js,jsx,ts,tsx}",
        "next-env.d.ts",
    ],
    production: &[
        "{,src/}app/{,[(]*[)]/}{manifest,robots}.{js,ts}",
        "{,src/}app/**/sitemap.{js,ts}",
        "{,src/}app/**/{icon,apple-icon,opengraph-image,twitter-image}.{js,jsx,ts,tsx}",
        "{,src/}{instrumentation,instrumentation-client,middleware,proxy}.{js,jsx,ts,tsx}",
        "{,src/}app/global-{error,not-found}.{js,jsx,ts,tsx}",
        "{,src/}app/**/{default,error,forbidden,loading,not-found,unauthorized}.{js,jsx,ts,tsx}",
        "{,src/}app/**/{layout,page,route,template}.{js,jsx,ts,tsx}",
        "{,src/}pages/**/*.{js,jsx,ts,tsx}",
    ],
    binaries: &["next"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Next {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// A custom `pageExtensions` replaces the default extension group of the router patterns.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let extensions: Vec<&str> = config
            .value
            .get("pageExtensions")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.trim_start_matches('.'))
            .filter(|e| !e.is_empty())
            .collect();
        if extensions.is_empty() {
            return framework_dependencies();
        }
        let group = format!("{{{}}}", extensions.join(","));
        let router = [
            format!("{{,src/}}app/**/{{default,error,forbidden,loading,not-found,unauthorized}}.{group}"),
            format!("{{,src/}}app/**/{{layout,page,route,template}}.{group}"),
            format!("{{,src/}}{{instrumentation,instrumentation-client,middleware,proxy}}.{group}"),
            format!("{{,src/}}pages/**/*.{group}"),
        ];
        router
            .into_iter()
            .map(|p| PluginInput::ProductionEntry(config.join(&p)))
            .chain(framework_dependencies())
            .collect()
    }

    fn resolve_without_config(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        framework_dependencies()
    }
}

/// Next.js renders with React and requires `react-dom` as a peer without any file importing it.
fn framework_dependencies() -> Vec<PluginInput> {
    ["react", "react-dom"]
        .into_iter()
        .map(|name| PluginInput::Dependency(name.to_owned()))
        .collect()
}
