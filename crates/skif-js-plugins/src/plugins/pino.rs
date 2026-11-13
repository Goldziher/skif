//! pino: the transports a source file names in `pino({ transport })` or `pino.transport({ … })` are loaded in a worker
//! by name. A package is a dependency; a relative path is an entry.

use skif_js_syntax::{ConfigValue, extract_imported_calls};

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, objects, parent_dir, sources_containing};

/// The `pino` plugin.
pub struct Pino;

static SPEC: PluginSpec = PluginSpec {
    id: "pino",
    enablers: &[Enabler::Dependency("pino")],
    ..PluginSpec::DEFAULT
};

impl Plugin for Pino {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for (source, text) in sources_containing(cx, "pino") {
            let path = cx.dir.join(&source);
            for call in extract_imported_calls(&text, &path, &["pino"], &[]) {
                let options = match call.export.as_str() {
                    "default" => call.arguments.first().and_then(|options| options.get("transport")),
                    "transport" => call.arguments.first(),
                    _ => None,
                };
                if let Some(options) = options {
                    targets(options, parent_dir(&source), &mut out, 0);
                }
            }
        }
        out
    }
}

/// The `target` of a transport and of each transport in its `targets` and `pipeline`.
fn targets(options: &ConfigValue, dir: &str, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 8 {
        return;
    }
    if let Some(target) = options.get("target").and_then(ConfigValue::as_str) {
        out.extend(module_input(dir, target, false));
    }
    for key in ["targets", "pipeline"] {
        for nested in options.get(key).map_or_default(objects) {
            targets(nested, dir, out, depth + 1);
        }
    }
}
