//! vite-plus: the commands of the `run.tasks` and the `staged` hooks written in `vite.config.*` are scripts.

use skif_js_syntax::{ConfigValue, extract_property_values};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{strings, values_of};

/// The `vite-plus` plugin.
pub struct VitePlus;

static SPEC: PluginSpec = PluginSpec {
    id: "vite-plus",
    enablers: &[Enabler::Dependency("vite-plus")],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for VitePlus {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read wherever they are written in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut commands: Vec<String> = Vec::new();
        for run in extract_property_values(config.source, config.path, "run") {
            let Some(tasks @ ConfigValue::Object(_)) = run.get("tasks") else {
                continue;
            };
            for task in values_of(tasks) {
                if matches!(task, ConfigValue::Object(_))
                    && let Some(command) = task.get("command")
                {
                    commands.extend(strings(command).into_iter().map(str::to_owned));
                }
            }
        }
        for staged in extract_property_values(config.source, config.path, "staged") {
            if matches!(staged, ConfigValue::Object(_)) {
                commands.extend(
                    values_of(&staged)
                        .into_iter()
                        .filter_map(ConfigValue::as_str)
                        .map(str::to_owned),
                );
            }
        }
        commands.into_iter().map(PluginInput::Script).collect()
    }
}
