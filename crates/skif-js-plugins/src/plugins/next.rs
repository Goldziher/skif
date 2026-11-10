//! Next.js: the file-system router entry points, a custom `pageExtensions`, and the aliases and loaders of the
//! `turbopack` settings.

use skif_js_syntax::ConfigValue;

use crate::plugin::{
    ConfigAst, Enabler, IncompleteCause, IncompleteScope, Plugin, PluginContext, PluginInput, PluginSpec,
};
use crate::support::{alias_inputs, convention_scope, get_in, module_input, package_name, strings, values_of};

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

    /// A custom `pageExtensions` replaces the default extension group of the router patterns. The packages that
    /// `turbopack.resolveAlias` points to and the loaders of `turbopack.rules` are dependencies, as knip has them;
    /// an alias to a path is a module alias.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = router_entries(config);
        if let Some(alias) = get_in(config.value, &["turbopack", "resolveAlias"]) {
            out.extend(alias_inputs(alias, config));
            for target in values_of(alias) {
                alias_targets(target, &mut out);
            }
        }
        if let Some(rules) = get_in(config.value, &["turbopack", "rules"]) {
            for rule in values_of(rules) {
                loaders(rule, config, &mut out, 0);
            }
        }
        out.extend(framework_dependencies());
        out
    }

    fn resolve_without_config(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        framework_dependencies()
    }

    /// Next.js takes its entries from the file-system router; a config it cannot read can change which files there
    /// are routes (`pageExtensions`), not add entries elsewhere.
    fn incomplete_scope(&self, _cause: IncompleteCause, config: &ConfigAst<'_>) -> IncompleteScope {
        convention_scope(
            &SPEC,
            config,
            &[
                "{,src/}{app,pages}/**",
                "{,src/}{instrumentation,instrumentation-client,middleware,proxy}.*",
            ],
        )
    }
}

/// The router patterns with the extension group of a custom `pageExtensions`; none without one, as the plugin's
/// default entries cover the default extensions.
fn router_entries(config: &ConfigAst<'_>) -> Vec<PluginInput> {
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
        return Vec::new();
    }
    let group = format!("{{{}}}", extensions.join(","));
    [
        format!("{{,src/}}app/**/{{default,error,forbidden,loading,not-found,unauthorized}}.{group}"),
        format!("{{,src/}}app/**/{{layout,page,route,template}}.{group}"),
        format!("{{,src/}}{{instrumentation,instrumentation-client,middleware,proxy}}.{group}"),
        format!("{{,src/}}pages/**/*.{group}"),
    ]
    .into_iter()
    .map(|p| PluginInput::ProductionEntry(config.join(&p)))
    .collect()
}

/// The packages an alias target names: a package name, a list of them, or the `browser` target of a conditional
/// one. Path targets are left to [`alias_inputs`].
fn alias_targets(target: &ConfigValue, out: &mut Vec<PluginInput>) {
    match target {
        ConfigValue::String(specifier) => {
            if let Some(name) = package_name(specifier) {
                out.push(PluginInput::Dependency(name));
            }
        }
        ConfigValue::Array(items) => items.iter().for_each(|item| alias_targets(item, out)),
        ConfigValue::Object(_) => {
            if let Some(browser) = target.get("browser") {
                alias_targets(browser, out);
            }
        }
        _ => {}
    }
}

/// The loaders of a `turbopack.rules` entry: a loader name, `{ loader }`, `{ loaders }` or a list of any of them.
fn loaders(rule: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 6 {
        return;
    }
    match rule {
        ConfigValue::String(specifier) => out.extend(module_input(config.base, specifier, false)),
        ConfigValue::Array(items) => items.iter().for_each(|item| loaders(item, config, out, depth + 1)),
        ConfigValue::Object(_) => {
            for key in ["loader", "loaders"] {
                if let Some(value) = rule.get(key) {
                    loaders(value, config, out, depth + 1);
                }
            }
        }
        _ => {}
    }
}

/// Next.js renders with React and requires `react-dom` as a peer without any file importing it.
fn framework_dependencies() -> Vec<PluginInput> {
    ["react", "react-dom"]
        .into_iter()
        .map(|name| PluginInput::Dependency(name.to_owned()))
        .collect()
}
