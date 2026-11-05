//! rsbuild: the source entries of the config and its environments are production entries; aliases, the `swc` plugins
//! and the `rspack` section follow the bundler.
//!
//! A config function (`defineConfig(({ env }) => …)`) is evaluated once without parameters, where knip calls it for
//! development and for production; a branch on `env` or `command` is not known. A function in `tools.rspack` is not
//! called, and an object it would return is not read.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::swc::plugin_packages;
use crate::plugins::webpack::bundler_inputs;
use crate::support::{alias_inputs, get_in, leaf_strings, objects, production_entries, strings, values_of};

/// The `rsbuild` plugin.
pub struct Rsbuild;

static SPEC: PluginSpec = PluginSpec {
    id: "rsbuild",
    enablers: &[Enabler::Dependency("@rsbuild/core")],
    config: &["rsbuild*.config.{mjs,ts,js,cjs,mts,cts}"],
    binaries: &["rsbuild"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Rsbuild {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut sections = objects(config.value);
        if let Some(environments) = config.value.get("environments") {
            sections.extend(values_of(environments).into_iter().flat_map(objects));
        }
        let mut out = Vec::new();
        for section in sections {
            out.extend(production_entries(config.base, source_entries(section)));
            if let Some(alias) = get_in(section, &["resolve", "alias"]) {
                out.extend(alias_inputs(alias, config));
            }
            if let Some(swc) = get_in(section, &["tools", "swc"]) {
                out.extend(
                    objects(swc)
                        .into_iter()
                        .flat_map(plugin_packages)
                        .map(PluginInput::OptionalDependency),
                );
            }
            if let Some(rspack) = get_in(section, &["tools", "rspack"]) {
                out.extend(
                    objects(rspack)
                        .into_iter()
                        .flat_map(|raw| bundler_inputs(&sub_config(config, raw), cx)),
                );
            }
        }
        out
    }
}

/// The `source.entry` values (a path, a list of paths or an object with an `import`) and `source.preEntry`.
fn source_entries(section: &ConfigValue) -> Vec<&str> {
    let mut found = Vec::new();
    if let Some(entry) = get_in(section, &["source", "entry"]) {
        for value in values_of(entry) {
            match value.get("import") {
                Some(import) => found.extend(strings(import)),
                None => found.extend(leaf_strings(value)),
            }
        }
    }
    if let Some(pre_entry) = get_in(section, &["source", "preEntry"]) {
        found.extend(strings(pre_entry));
    }
    found
}

/// The config of the same file with another value, for the resolver of a nested section.
fn sub_config<'a>(config: &ConfigAst<'a>, value: &'a ConfigValue) -> ConfigAst<'a> {
    ConfigAst { value, ..*config }
}
