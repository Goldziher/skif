//! nano-spawn: the commands a source file runs with `spawn('eslint', ['.'])` are scripts, parsed like the scripts of
//! `package.json`. Only literal commands count.

use skif_js_syntax::extract_command_calls;

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::execa::safe_command;
use crate::support::sources_containing;

/// The `nano-spawn` plugin.
pub struct NanoSpawn;

static SPEC: PluginSpec = PluginSpec {
    id: "nano-spawn",
    enablers: &[Enabler::Dependency("nano-spawn")],
    ..PluginSpec::DEFAULT
};

impl Plugin for NanoSpawn {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for (source, text) in sources_containing(cx, "nano-spawn") {
            let path = cx.dir.join(&source);
            for call in extract_command_calls(&text, &path, &["nano-spawn"], &[]) {
                if call.export == "default" && call.template.is_none() {
                    out.extend(safe_command(&call.arguments).map(|script| PluginInput::SourceScript {
                        file: source.clone(),
                        script,
                    }));
                }
            }
        }
        out
    }
}
