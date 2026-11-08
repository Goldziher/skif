//! Flue: the `app`, `db` and `cloudflare` modules and the agents of a Flue project are production entries, at the
//! paths `flue.config.*` writes or by convention under `.flue/`, `src/` or the project directory.

use skif_js_syntax::extract_property_values;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::join_rel;

/// The `flue` plugin.
pub struct Flue;

const EXTENSIONS: &str = "{ts,mts,js,mjs}";
const AGENTS: &str = "{agent,agents/*,agents/*/agent}.{ts,mts,js,mjs}";

static SPEC: PluginSpec = PluginSpec {
    id: "flue",
    enablers: &[Enabler::Dependency("@flue/runtime")],
    config: &["flue.config.{ts,mts,mjs,js,cjs,cts}"],
    production: &[
        "{.flue/,src/,}{app,db,cloudflare}.{ts,mts,js,mjs}",
        "{.flue/,src/,}{agent,agents/*,agents/*/agent}.{ts,mts,js,mjs}",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for Flue {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read wherever they are written in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let source_root = [".flue", "src"]
            .into_iter()
            .map(|name| config.join(name))
            .find(|dir| cx.fs.is_dir(&cx.dir.join(dir)))
            .unwrap_or_else(|| config.base.to_owned());
        let first = |key: &str| {
            extract_property_values(config.source, config.path, key)
                .into_iter()
                .find_map(|value| value.as_str().map(str::to_owned))
        };
        let mut out: Vec<PluginInput> = ["app", "db", "cloudflare"]
            .into_iter()
            .map(|name| match first(name) {
                Some(path) => PluginInput::ProductionEntry(config.join(&path)),
                None => PluginInput::ProductionEntry(join_rel(&source_root, &format!("{name}.{EXTENSIONS}"))),
            })
            .collect();
        let agents = first("agents").unwrap_or_else(|| AGENTS.to_owned());
        out.push(PluginInput::ProductionEntry(join_rel(&source_root, &agents)));
        out
    }
}
