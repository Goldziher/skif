//! filesystem-routing: the route files under the `dir` of `filesystemRouting({ … })` from `filesystem-routing/vite`
//! in `vite.config.*` (`src/routes` under the Vite root by default), with its `extensions`, are production entries.

use skif_js_syntax::{ConfigValue, extract_imported_calls};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, vite_root};

/// The `filesystem-routing` plugin.
pub struct FilesystemRouting;

const DEFAULT_DIR: &str = "src/routes";
const DEFAULT_EXTENSIONS: &[&str] = &["js", "jsx", "ts", "tsx", "tsrx"];

static SPEC: PluginSpec = PluginSpec {
    id: "filesystem-routing",
    enablers: &[Enabler::Dependency("filesystem-routing")],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    production: &["src/routes/**/*.{js,jsx,ts,tsx,tsrx}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for FilesystemRouting {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the plugin call in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let root = vite_root(config);
        let calls = extract_imported_calls(config.source, config.path, &["filesystem-routing/vite"], &[]);
        if calls.is_empty() {
            return SPEC
                .production
                .iter()
                .map(|pattern| PluginInput::ProductionEntry(join_rel(&root, pattern)))
                .collect();
        }
        let mut out = Vec::new();
        for call in calls {
            let options = call.arguments.first();
            let dir = options
                .and_then(|options| options.get("dir"))
                .and_then(ConfigValue::as_str)
                .unwrap_or(DEFAULT_DIR);
            let mut extensions: Vec<&str> = Vec::new();
            let mut defaults = true;
            if let Some(ConfigValue::Array(items)) = options.and_then(|options| options.get("extensions")) {
                defaults = false;
                for item in items {
                    match item.as_str() {
                        Some(extension) => extensions.push(extension),
                        None => defaults = true,
                    }
                }
            }
            if defaults {
                extensions.extend(DEFAULT_EXTENSIONS);
            }
            extensions.dedup();
            let suffix = match extensions.as_slice() {
                [] => continue,
                [one] => (*one).to_owned(),
                many => format!("{{{}}}", many.join(",")),
            };
            out.push(PluginInput::ProductionEntry(join_rel(
                &join_rel(&root, dir),
                &format!("**/*.{suffix}"),
            )));
        }
        out
    }
}
