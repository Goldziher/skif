//! Prisma: the schema (a file or a directory of files) and the seed command.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `prisma` plugin.
pub struct Prisma;

static SPEC: PluginSpec = PluginSpec {
    id: "prisma",
    enablers: &[Enabler::Dependency("prisma"), Enabler::DependencyPrefix("@prisma/")],
    config: &[
        "prisma.config.{js,ts,mjs,cjs,mts,cts}",
        ".config/prisma.{js,ts,mjs,cjs,mts,cts}",
    ],
    package_json_key: Some("prisma"),
    entry: &["prisma/schema.prisma", "schema.prisma"],
    binaries: &["prisma"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Prisma {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let seed = config
            .value
            .get("seed")
            .or_else(|| config.value.get("migrations").and_then(|m| m.get("seed")))
            .and_then(ConfigValue::as_str);
        out.extend(seed.map(|seed| PluginInput::Script(seed.to_owned())));
        if let Some(schema) = config.value.get("schema").and_then(ConfigValue::as_str) {
            let path = config.join(schema);
            out.push(PluginInput::Entry(if path.ends_with(".prisma") {
                path
            } else {
                format!("{}/**/*.prisma", path.trim_end_matches('/'))
            }));
        }
        out
    }
}
