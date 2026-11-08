//! `@sveltejs/package`: `svelte-package` builds the library directory (`-i`, `src/lib` by default) into the output
//! directory (`-o`, `dist` by default), so imports of the output lead to the sources.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{flag_values, workspace_path};

/// The `sveltejs-package` plugin.
pub struct SveltejsPackage;

static SPEC: PluginSpec = PluginSpec {
    id: "sveltejs-package",
    enablers: &[Enabler::Dependency("@sveltejs/package")],
    binaries: &["svelte-package"],
    ..PluginSpec::DEFAULT
};

const DEFAULT_INPUT: &str = "src/lib";
const DEFAULT_OUTPUT: &str = "dist";

impl Plugin for SveltejsPackage {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for script in cx.scripts.values() {
            if !script.contains(SPEC.binaries[0]) {
                continue;
            }
            // One pair per command; `flag_values` reads every command of the script, so read each command alone.
            for command in script.split(['&', '|', ';', '\n']) {
                if !command.contains(SPEC.binaries[0]) {
                    continue;
                }
                let flag = |flags: &[&str], default: &str| {
                    flag_values(command, SPEC.binaries, flags)
                        .last()
                        .and_then(|value| workspace_path(value))
                        .unwrap_or_else(|| default.to_owned())
                };
                let pair = (
                    flag(&["-i", "--input"], DEFAULT_INPUT),
                    flag(&["-o", "--output"], DEFAULT_OUTPUT),
                );
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
        if pairs.is_empty() {
            pairs.push((DEFAULT_INPUT.to_owned(), DEFAULT_OUTPUT.to_owned()));
        }
        pairs
            .into_iter()
            .map(|(root_dir, out_dir)| PluginInput::BuildDir { out_dir, root_dir })
            .collect()
    }
}
