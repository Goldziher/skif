//! Knex: the database driver of `client`, migration and seed files.
//!
//! Like knip's source visitor, the source files of the workspace are searched for `knex({ client: "pg" })` calls (the
//! default import, the named `knex` import and the `require` forms of both); the driver of a literal `client` is a
//! dependency. The search reads the text, not the syntax tree: it follows a call only when its first argument is an
//! object literal with a string literal `client`, and does not see a `knex` binding that is renamed again.

use skif_js_syntax::ConfigValue;

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, literal_property, strings, values_of};

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

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let patterns = ["**/*.{ts,mts,cts,tsx,js,mjs,cjs,jsx}".to_owned()];
        let mut out = Vec::new();
        for source in find_files(cx.fs, cx.globs, cx.dir, &patterns) {
            if source.ends_with(".d.ts") {
                continue;
            }
            let Ok(text) = cx.fs.read_to_string(&cx.dir.join(&source)) else {
                continue;
            };
            if !text.contains("knex") {
                continue;
            }
            for client in source_clients(&text) {
                let packages = driver_packages(&client.to_ascii_lowercase());
                out.extend(packages.iter().map(|name| PluginInput::Dependency((*name).to_owned())));
            }
        }
        out
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

/// The literal `client` of every `knex({ client: "…" })` call in a source text.
fn source_clients(text: &str) -> Vec<&str> {
    let (defaults, named) = bindings(text);
    let mut clients = Vec::new();
    for name in &defaults {
        for call in [format!("{name}.knex"), name.clone()] {
            clients.extend(call_clients(text, &call));
        }
    }
    for name in &named {
        clients.extend(call_clients(text, name));
    }
    clients
}

/// The local names bound to the default export and to the named `knex` export of the `knex` package.
fn bindings(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut defaults, mut named) = (Vec::new(), Vec::new());
    for quote in ["'knex'", "\"knex\""] {
        for (at, _) in text.match_indices(quote) {
            let before = text[..at].trim_end();
            if let Some(head) = before.strip_suffix("from") {
                let Some(start) = head.rfind("import") else {
                    continue;
                };
                let clause = head[start + "import".len()..].trim().trim_start_matches("type ");
                let (default, braces) = clause.split_once('{').map_or((clause, ""), |(d, b)| (d, b));
                defaults.extend(trailing_identifier(default.trim().trim_end_matches(',').trim()));
                for item in braces.trim_end_matches('}').split(',') {
                    let mut parts = item.split_whitespace().filter(|part| *part != "type");
                    match (parts.next(), parts.next(), parts.next()) {
                        (Some("knex"), Some("as"), Some(local)) => named.push(local.to_owned()),
                        (Some("knex"), None, None) => named.push("knex".to_owned()),
                        _ => {}
                    }
                }
            } else if let Some(head) = before.strip_suffix("require(") {
                let Some(target) = head.trim_end().strip_suffix('=') else {
                    continue;
                };
                let target = target.trim_end();
                if let Some(pattern) = target.strip_suffix('}') {
                    let inner = pattern.rsplit_once('{').map_or("", |(_, inner)| inner);
                    for item in inner.split(',') {
                        let (key, local) = item
                            .split_once(':')
                            .map_or((item.trim(), item.trim()), |(k, v)| (k.trim(), v.trim()));
                        if key == "knex" {
                            named.extend(trailing_identifier(local));
                        }
                    }
                } else {
                    defaults.extend(trailing_identifier(target));
                }
            }
        }
    }
    (defaults, named)
}

/// The identifier a text ends with, with a type annotation (`knex: Foo`) ignored.
fn trailing_identifier(text: &str) -> Option<String> {
    let text = text.rsplit_once(':').map_or(text, |(name, _)| name).trim_end();
    let name: String = text
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The literal `client` of each `callee({ … })` call in a text.
fn call_clients<'t>(text: &'t str, callee: &str) -> Vec<&'t str> {
    let call = format!("{callee}(");
    let mut clients = Vec::new();
    for (at, _) in text.match_indices(&call) {
        let boundary = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '.')));
        let rest = &text[at + call.len()..];
        let argument = rest.trim_start();
        if boundary && argument.starts_with('{') {
            clients.extend(literal_property(argument, "client"));
        }
    }
    clients
}
