//! React Cosmos: fixtures and decorators are entries, under the `fixturesDir` and with the `fixtureFileSuffix` of
//! `cosmos.config.json`; its `plugins` are used.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, module_input, strings};

/// The `react-cosmos` plugin.
pub struct ReactCosmos;

const EXTENSIONS: &str = "{js,jsx,ts,tsx,md,mdx}";
const DECORATORS: &str = "**/cosmos.decorator.{jsx,tsx}";

static SPEC: PluginSpec = PluginSpec {
    id: "react-cosmos",
    enablers: &[Enabler::Dependency("react-cosmos")],
    config: &["cosmos.config.json"],
    entry: &[
        "**/*.fixture.{js,jsx,ts,tsx,md,mdx}",
        "__fixtures__/**/*.{js,jsx,ts,tsx,md,mdx}",
        "**/fixture.{js,jsx,ts,tsx,md,mdx}",
        DECORATORS,
    ],
    binaries: &["cosmos", "cosmos-export"],
    ..PluginSpec::DEFAULT
};

impl Plugin for ReactCosmos {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let text = |key: &str| config.value.get(key).and_then(ConfigValue::as_str);
        let dir = text("fixturesDir");
        let suffix = text("fixtureFileSuffix").unwrap_or("fixture");
        let mut out: Vec<PluginInput> = [
            join_rel(dir.unwrap_or("__fixtures__"), &format!("**/*.{EXTENSIONS}")),
            join_rel(dir.unwrap_or(""), &format!("**/*.{suffix}.{EXTENSIONS}")),
            join_rel(dir.unwrap_or(""), &format!("**/{suffix}.{EXTENSIONS}")),
            DECORATORS.to_owned(),
        ]
        .into_iter()
        .map(PluginInput::Entry)
        .collect();
        out.extend(
            config
                .value
                .get("plugins")
                .map_or_default(strings)
                .into_iter()
                .filter_map(|id| module_input("", id, false)),
        );
        out
    }
}
