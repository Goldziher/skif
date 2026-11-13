//! Biome: a config it extends is a further config file or a shared config package, and `"//"` is the root config of
//! the repository, found as the `biome.json` or `biome.jsonc` of the farthest ancestor directory that has one (the
//! plugin does not know where the repository root is).

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, join_rel, strings};

/// The `biome` plugin.
pub struct Biome;

static SPEC: PluginSpec = PluginSpec {
    id: "biome",
    enablers: &[Enabler::Dependency("@biomejs/biome")],
    config: &["biome.json", "biome.jsonc", ".biome.json", ".biome.jsonc"],
    binaries: &["biome"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Biome {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("extends")
            .map_or_default(strings)
            .into_iter()
            .filter_map(|specifier| {
                if specifier == "//" {
                    root_config(config, cx)
                } else {
                    config_ref(config.base, specifier)
                }
            })
            .collect()
    }
}

/// How many directories above the config file the root config is looked for.
const MAX_ANCESTORS: usize = 8;

/// The config file of the farthest ancestor directory of the config file that has one.
fn root_config(config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Option<PluginInput> {
    let directory = config.path.parent()?;
    let mut found = None;
    for (depth, ancestor) in directory.ancestors().enumerate().take(MAX_ANCESTORS + 1).skip(1) {
        for name in ["biome.json", "biome.jsonc"] {
            if cx.fs.is_file(&ancestor.join(name)) {
                found = Some(join_rel(config.base, &format!("{}{name}", "../".repeat(depth))));
            }
        }
    }
    found.map(PluginInput::ConfigFile)
}
