//! `@solidjs/vite-plugin`: the app files of `solid({ start })` in `vite.config.*`, at the paths `start` writes or by
//! convention under the Vite root, are production entries; which conventions apply follows the `ssr` option. The
//! `serverFunctions.configure` module is one too.

use skif_js_syntax::{ConfigValue, extract_imported_calls};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, vite_root};

/// The `solidjs-vite-plugin` plugin.
pub struct SolidjsVitePlugin;

const ENTRY_EXTENSIONS: &[&str] = &["tsx", "jsx", "ts", "js", "mjs", "tsrx"];

/// The start options, in order, with the convention of those that have one.
const START_OPTIONS: &[(&str, Option<&str>)] = &[
    ("app", Some("src/{App,app}.{tsx,jsx,ts,js,tsrx}")),
    ("document", Some("src/Document.{tsx,jsx,tsrx}")),
    ("entryServer", Some("src/entry-server.{tsx,jsx,ts,js,mjs,tsrx}")),
    ("entryClient", Some("src/entry-client.{tsx,jsx,ts,js,mjs,tsrx}")),
    ("middleware", None),
    ("instrument", None),
    ("setup", None),
    ("env", Some("env.{ts,js}")),
];

static SPEC: PluginSpec = PluginSpec {
    id: "solidjs-vite-plugin",
    enablers: &[Enabler::Dependency("@solidjs/vite-plugin")],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    production: &[
        "src/entry-server.{tsx,jsx,ts,js,mjs,tsrx}",
        "src/entry-client.{tsx,jsx,ts,js,mjs,tsrx}",
        "src/{App,app}.{tsx,jsx,ts,js,tsrx}",
        "src/Document.{tsx,jsx,tsrx}",
        "env.{ts,js}",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for SolidjsVitePlugin {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the plugin call in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let root = vite_root(config);
        let has_entry_file = |stem: &str| {
            ENTRY_EXTENSIONS.iter().any(|extension| {
                cx.fs
                    .is_file(&cx.dir.join(join_rel(&root, &format!("{stem}.{extension}"))))
            })
        };
        let mut patterns: Vec<&str> = Vec::new();
        let mut paths: Vec<&str> = Vec::new();
        let calls = extract_imported_calls(config.source, config.path, &["@solidjs/vite-plugin"], &[]);
        for options in calls.iter().filter_map(|call| call.arguments.first()) {
            if !matches!(options, ConfigValue::Object(_)) {
                continue;
            }
            if let Some(configure) = options
                .get("serverFunctions")
                .and_then(|functions| functions.get("configure"))
                .and_then(ConfigValue::as_str)
            {
                paths.push(configure);
            }
            let Some(start) = options.get("start").filter(|start| start.as_bool() != Some(false)) else {
                continue;
            };
            let value = |name: &str| start.get(name);
            let authored =
                |name: &str, stem: &str| value(name).and_then(ConfigValue::as_str).is_some() || has_entry_file(stem);
            let ssr = options.get("ssr");
            let mut skip: Vec<&str> = Vec::new();
            if ssr.is_none_or(|ssr| ssr.as_bool() == Some(false)) {
                skip.extend(["entryServer", "setup"]);
                if authored("entryClient", "src/entry-client") {
                    skip.push("app");
                }
            } else if ssr.and_then(ConfigValue::as_bool) == Some(true)
                && authored("entryServer", "src/entry-server")
                && authored("entryClient", "src/entry-client")
            {
                skip.extend(["app", "document"]);
            }
            for (name, convention) in START_OPTIONS {
                if skip.contains(name) {
                    continue;
                }
                match value(name) {
                    Some(ConfigValue::String(path)) => paths.push(path),
                    other => {
                        if let Some(convention) = convention
                            && !(*name == "env" && other.and_then(ConfigValue::as_bool) == Some(false))
                        {
                            patterns.push(convention);
                        }
                    }
                }
            }
        }
        patterns.dedup();
        patterns
            .into_iter()
            .chain(paths)
            .map(|path| PluginInput::ProductionEntry(join_rel(&root, path)))
            .collect()
    }
}
