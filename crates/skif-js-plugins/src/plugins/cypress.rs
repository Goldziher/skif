//! Cypress: spec and support files of the e2e and component configs, and the defaults of Nx's `nxE2EPreset()`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, strings};

/// The `cypress` plugin.
pub struct Cypress;

/// The plugin's identifier.
pub const ID: crate::plugin::PluginId = "cypress";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[Enabler::Dependency("cypress")],
    config: &["cypress.config.{js,ts,mjs,cjs}"],
    entry: &[
        "cypress/e2e/**/*.cy.{js,jsx,ts,tsx}",
        "cypress/support/e2e.{js,jsx,ts,tsx}",
        "cypress/support/commands.{js,ts}",
        "cypress/support/component.{js,ts}",
        "cypress/plugins/index.js",
    ],
    binaries: &["cypress"],
    ..PluginSpec::DEFAULT
};

/// The modules Nx's `nxE2EPreset()` comes from.
const NX_PRESET_MODULES: &[&str] = &[
    "@nx/cypress/plugins/cypress-preset",
    "@nrwl/cypress/plugins/cypress-preset",
];

/// The keys `nxE2EPreset()` sets, with their value below its `cypressDir`.
const NX_PRESET_KEYS: &[(&str, &str)] = &[
    ("specPattern", "**/*.cy.{js,jsx,ts,tsx}"),
    ("supportFile", "support/e2e.{js,ts}"),
];

/// The `cypressDir` of `nxE2EPreset()` when its options give none.
const NX_DEFAULT_DIR: &str = "src";

impl Plugin for Cypress {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut patterns: Vec<String> = Vec::new();
        for kind in ["e2e", "component"] {
            let Some(section) = config.value.get(kind) else {
                continue;
            };
            if kind == "e2e" && spreads_nx_preset(section, config.source) {
                patterns.extend(nx_preset_patterns(section, config.source));
                continue;
            }
            for key in ["specPattern", "supportFile"] {
                patterns.extend(section.get(key).into_iter().flat_map(strings).map(str::to_owned));
            }
        }
        patterns
            .iter()
            .filter_map(|pattern| entry_glob(config.base, pattern))
            .map(PluginInput::Entry)
            .collect()
    }
}

/// Whether `section` spreads an unknown value and the config imports `nxE2EPreset` from [`NX_PRESET_MODULES`] and
/// calls it. The call is recognised in the source, as the evaluator keeps no trace of an unknown value's origin.
fn spreads_nx_preset(section: &ConfigValue, source: &str) -> bool {
    let ConfigValue::Object(entries) = section else {
        return false;
    };
    let imported = NX_PRESET_MODULES.iter().any(|module| {
        ['\'', '"', '`']
            .iter()
            .any(|quote| source.contains(&format!("{quote}{module}{quote}")))
    });
    imported
        && entries.iter().any(|(key, _)| ConfigValue::is_spread_key(key))
        && source.match_indices("nxE2EPreset").any(|(at, name)| {
            source[at + name.len()..].trim_start().starts_with('(')
        })
}

/// The spec and support patterns of an e2e section that spreads `nxE2EPreset()`: a key written after the last spread
/// overrides the preset, any other is the preset's value below its `cypressDir`.
fn nx_preset_patterns(section: &ConfigValue, source: &str) -> Vec<String> {
    let ConfigValue::Object(entries) = section else {
        return Vec::new();
    };
    let dir = nx_cypress_dir(source).unwrap_or(NX_DEFAULT_DIR);
    let mut patterns = Vec::new();
    for (key, default) in NX_PRESET_KEYS {
        let last = entries
            .iter()
            .rev()
            .find(|(name, _)| name == key || ConfigValue::is_spread_key(name));
        match last {
            Some((name, value)) if name == key => patterns.extend(strings(value).into_iter().map(str::to_owned)),
            _ => patterns.push(format!("{dir}/{default}")),
        }
    }
    patterns
}

/// The `cypressDir` option of the preset when the source writes it as a string literal (`cypressDir: 'cypress'`).
fn nx_cypress_dir(source: &str) -> Option<&str> {
    let (_, rest) = source.split_once("cypressDir")?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"' | '`'))?;
    let rest = &rest[quote.len_utf8()..];
    let value = rest[..rest.find(quote)?].trim_end_matches('/');
    (!value.is_empty() && !value.contains("${")).then_some(value)
}
