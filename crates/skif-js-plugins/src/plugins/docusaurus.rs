//! Docusaurus: pages and docs, and the themes, plugins and presets the config names.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, join_rel, name_of, package_name};

/// The `docusaurus` plugin.
pub struct Docusaurus;

static SPEC: PluginSpec = PluginSpec {
    id: "docusaurus",
    enablers: &[Enabler::Dependency("@docusaurus/core")],
    config: &["docusaurus.config.{js,cjs,mjs,ts,cts,mts}"],
    production: &[
        "src/pages/**/*.{js,ts,jsx,tsx}",
        "{blog,docs}/**/*.mdx",
        "versioned_docs/**/*.{mdx,jsx,tsx}",
    ],
    binaries: &["docusaurus"],
    ..PluginSpec::DEFAULT
};

/// The Babel config Docusaurus projects customise.
const BABEL_CONFIG: &str = "babel.config.{js,cjs,mjs,json,cts,mts,ts}";

/// The `@docusaurus/core` client API, which `@docusaurus/core` aliases to files of its own package.
const CORE_CLIENT_API: &[&str] = &[
    "BrowserOnly",
    "ComponentCreator",
    "constants",
    "ExecutionEnvironment",
    "Head",
    "Interpolate",
    "isInternalUrl",
    "Link",
    "Noop",
    "renderRoutes",
    "router",
    "Translate",
    "useBaseUrl",
    "useBrokenLinks",
    "useDocusaurusContext",
    "useGlobalData",
    "useIsBrowser",
    "useIsomorphicLayoutEffect",
    "useRouteContext",
];

impl Plugin for Docusaurus {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = vec![
            PluginInput::LinkedConfig {
                plugin: "babel",
                path: BABEL_CONFIG.to_owned(),
            },
            PluginInput::Alias {
                find: "@site".to_owned(),
                target: Some(".".to_owned()),
            },
            PluginInput::OptionalDependency("@docusaurus/module-type-aliases".to_owned()),
            // Modules Docusaurus generates at build time.
            PluginInput::VirtualModule("@generated/*".to_owned()),
            PluginInput::VirtualModule(format!("@docusaurus/{{{}}}", CORE_CLIENT_API.join(","))),
        ];
        let classic = ["@docusaurus/theme-classic", "@docusaurus/preset-classic"]
            .iter()
            .any(|name| cx.has_dependency(name));
        if classic {
            out.extend(
                ["@theme/*", "@theme-init/*", "@theme-original/*"]
                    .into_iter()
                    .map(|pattern| PluginInput::VirtualModule(pattern.to_owned())),
            );
        }
        out
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for (key, kind) in [("themes", "theme"), ("plugins", "plugin"), ("presets", "preset")] {
            let Some(ConfigValue::Array(items)) = config.value.get(key) else {
                continue;
            };
            for name in items.iter().filter_map(name_of) {
                if is_relative(name) {
                    out.push(PluginInput::Entry(join_rel(config.base, name)));
                } else {
                    out.extend(package_name(&module_name(name, kind)).map(PluginInput::Dependency));
                }
            }
        }
        if config
            .value
            .get("future")
            .and_then(|f| f.get("experimental_faster"))
            .is_some()
        {
            out.push(PluginInput::Dependency("@docusaurus/faster".to_owned()));
        }
        out
    }
}

/// Expands the shorthand `classic` to `@docusaurus/preset-classic`, `foo` to `docusaurus-plugin-foo` and
/// `@scope/foo` to `@scope/docusaurus-plugin-foo`.
fn module_name(name: &str, kind: &str) -> String {
    let prefix = format!("docusaurus-{kind}");
    match name.split_once('/') {
        _ if name == "classic" => format!("@docusaurus/{kind}-classic"),
        Some(("@docusaurus", rest)) if rest.starts_with(&format!("{kind}-")) => name.to_owned(),
        Some(("@docusaurus", rest)) => format!("@docusaurus/{kind}-{rest}"),
        Some((scope, rest)) if scope.starts_with('@') && !rest.starts_with(&prefix) => {
            format!("{scope}/{prefix}-{rest}")
        }
        None if name.starts_with('@') => format!("{name}/{prefix}"),
        None if !name.starts_with(&prefix) => format!("{prefix}-{name}"),
        _ => name.to_owned(),
    }
}
