//! Knex: the database driver of `client`, migration and seed files.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, strings, values_of};

/// The `knex` plugin.
pub struct Knex;

static SPEC: PluginSpec = PluginSpec {
    id: "knex",
    enablers: &[Enabler::Dependency("knex")],
    config: &["knexfile.{js,cjs,mjs,ts,cts,mts}"],
    binaries: &["knex"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Knex {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// A knexfile is one config or an object of configs by environment.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let environments = if config.value.get("client").is_some() {
            vec![config.value]
        } else {
            values_of(config.value)
                .into_iter()
                .filter(|v| matches!(v, ConfigValue::Object(_)))
                .collect()
        };
        environments
            .into_iter()
            .flat_map(|environment| environment_inputs(environment, config))
            .collect()
    }
}

fn environment_inputs(environment: &ConfigValue, config: &ConfigAst<'_>) -> Vec<PluginInput> {
    let mut out = Vec::new();
    if let Some(client) = environment.get("client").and_then(ConfigValue::as_str) {
        out.extend(
            driver_packages(&client.to_ascii_lowercase())
                .iter()
                .map(|name| PluginInput::OptionalDependency((*name).to_owned())),
        );
    }
    for section in ["migrations", "seeds"] {
        let directories = environment.get(section).and_then(|s| s.get("directory"));
        for directory in directories.map(strings).unwrap_or_default() {
            let pattern = format!("{}/*.{{js,ts}}", directory.trim_end_matches('/'));
            out.extend(entry_glob(config.base, &pattern).map(PluginInput::Entry));
        }
    }
    out
}

fn driver_packages(client: &str) -> &'static [&'static str] {
    match client {
        "pg" | "postgres" | "postgresql" | "cockroachdb" | "redshift" => &["pg"],
        "mysql" => &["mysql", "mysql2"],
        "mysql2" => &["mysql2"],
        "sqlite3" => &["sqlite3"],
        "better-sqlite3" => &["better-sqlite3"],
        "mssql" | "tedious" => &["tedious"],
        "oracledb" | "oracle" => &["oracledb"],
        _ => &[],
    }
}
