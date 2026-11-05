//! Nuxt: pages, layouts, middleware, plugins, modules and the server directory, plus the modules and layers the config
//! names.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, name_of, strings};
use skif_js_syntax::ConfigValue;

/// The `nuxt` plugin.
pub struct Nuxt;

static SPEC: PluginSpec = PluginSpec {
    id: "nuxt",
    enablers: &[Enabler::Dependency("nuxt"), Enabler::Dependency("nuxt-nightly")],
    config: &["nuxt.config.{js,cjs,mjs,ts,cts,mts}"],
    entry: &["{,app/}app.config.ts", "**/*.d.vue.ts"],
    production: &[
        "{,app/}{app,error}.{vue,jsx,tsx}",
        "{,app/}router.options.ts",
        "{,app/}layouts/**/*.{vue,jsx,tsx}",
        "{,app/}middleware/**/*.ts",
        "{,app/}pages/**/*.{vue,jsx,tsx}",
        "{,app/}plugins/**/*.ts",
        "{,app/}components/global/**/*.{vue,jsx,tsx}",
        "modules/**/*.{ts,vue}",
        "server/{api,middleware,plugins,routes,tasks}/**/*.ts",
    ],
    binaries: &["nuxt", "nuxi"],
    ..PluginSpec::DEFAULT
};

/// Layer sources fetched from a remote location rather than installed.
const REMOTE_PREFIXES: &[&str] = &["gh:", "github:", "gitlab:", "bitbucket:", "https://", "http://"];

/// Modules Nuxt generates, which no file backs.
const VIRTUAL_MODULES: &[&str] = &[
    "#app",
    "#app/*",
    "#build/*",
    "#components",
    "#imports",
    "#internal/*",
    "#spa-template",
];

impl Plugin for Nuxt {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        // Nuxt writes its aliases to `.nuxt/tsconfig.json`, which only exists after `nuxt prepare`.
        let source_dir = if cx.fs.is_dir(&cx.dir.join("app")) { "app" } else { "." };
        let mut out: Vec<PluginInput> = [("~", source_dir), ("@", source_dir), ("~~", "."), ("@@", ".")]
            .into_iter()
            .map(|(find, target)| PluginInput::Alias {
                find: find.to_owned(),
                target: Some(target.to_owned()),
            })
            .collect();
        out.extend(
            VIRTUAL_MODULES
                .iter()
                .map(|pattern| PluginInput::VirtualModule((*pattern).to_owned())),
        );
        out
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(modules) = config.value.get("modules") {
            out.extend(
                module_names(modules)
                    .into_iter()
                    .filter_map(|name| module_input(config.base, name, false)),
            );
        }
        if let Some(extended) = config.value.get("extends") {
            out.extend(
                module_names(extended)
                    .into_iter()
                    .filter(|source| !REMOTE_PREFIXES.iter().any(|prefix| source.starts_with(prefix)))
                    .filter_map(|source| module_input(config.base, source, false)),
            );
        }
        if let Some(css) = config.value.get("css") {
            out.extend(
                strings(css)
                    .into_iter()
                    .filter(|path| !path.starts_with('~') && !path.starts_with("@/"))
                    .filter_map(|path| module_input(config.base, path, false)),
            );
        }
        out
    }
}

/// Names written as `"name"` or `["name", options]`, also inside a list.
fn module_names(value: &ConfigValue) -> Vec<&str> {
    match value {
        ConfigValue::Array(items) => items
            .iter()
            .filter_map(|item| name_of(item).or_else(|| item.get("source").and_then(ConfigValue::as_str)))
            .collect(),
        other => name_of(other).into_iter().collect(),
    }
}
