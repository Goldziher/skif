//! Stencil: components under `srcDir`, spec and e2e tests, and the global script and styles, TypeScript config and
//! test setup files named anywhere in `stencil.config.*`. A class decorated with `@Component` from `@stencil/core` is
//! a registered custom element and so a used export (ADR 0062).

use skif_js_syntax::{ConfigValue, extract_property_values};

use crate::plugin::{ConfigAst, CustomElements, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{leaf_strings, objects};

/// The `stencil` plugin.
pub struct Stencil;

static SPEC: PluginSpec = PluginSpec {
    id: "stencil",
    enablers: &[Enabler::Dependency("@stencil/core")],
    config: &["stencil.config.{ts,js}"],
    entry: &["**/*.spec.{ts,tsx}", "**/*.e2e.{ts,tsx}"],
    production: &["src/**/*.tsx"],
    binaries: &["stencil"],
    custom_elements: Some(CustomElements {
        modules: &["@stencil/core"],
        decorator: "Component",
        base_class: None,
    }),
    ..PluginSpec::DEFAULT
};

impl Plugin for Stencil {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read wherever they are written in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let values = |key: &str| extract_property_values(config.source, config.path, key);
        let strings_of = |key: &str| -> Vec<String> {
            values(key)
                .iter()
                .flat_map(|value| leaf_strings(value).into_iter().map(str::to_owned).collect::<Vec<_>>())
                .collect()
        };
        let src_dir = strings_of("srcDir")
            .into_iter()
            .next()
            .unwrap_or_else(|| "src".to_owned());
        let mut out = vec![PluginInput::ProductionEntry(
            config.join(&format!("{src_dir}/**/*.tsx")),
        )];
        for key in ["globalScript", "globalStyle"] {
            out.extend(
                strings_of(key)
                    .iter()
                    .map(|file| PluginInput::ProductionEntry(config.join(file))),
            );
        }
        for target in values("outputTargets").iter().flat_map(objects) {
            if target.get("type").and_then(ConfigValue::as_str) == Some("global-style") {
                for input in target.get("input").map(leaf_strings).unwrap_or_default() {
                    out.push(PluginInput::ProductionEntry(config.join(input)));
                }
            }
        }
        for tsconfig in strings_of("tsconfig") {
            out.push(PluginInput::LinkedConfig {
                plugin: "typescript",
                path: config.join(&tsconfig),
            });
        }
        for key in ["setupFilesAfterEnv", "setupFiles"] {
            out.extend(strings_of(key).iter().map(|file| PluginInput::Entry(config.join(file))));
        }
        out
    }
}
