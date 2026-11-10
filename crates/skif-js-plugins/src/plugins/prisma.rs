//! Prisma: the schema (a file or a directory of files), the seed command and the generators the schema declares.
//!
//! A `generator` block's `provider` names the package that generates code (`prisma-kysely`, `zod-prisma-types`), a
//! command (`node ./gen.js`) or a path relative to the schema file. The built-in `prisma-client-js` and
//! `prisma-client` providers name no package.

use skif_js_syntax::ConfigValue;

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, join_rel, package_name};

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

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let seed = config
            .value
            .get("seed")
            .or_else(|| config.value.get("migrations").and_then(|m| m.get("seed")))
            .and_then(ConfigValue::as_str);
        out.extend(seed.map(|seed| PluginInput::Script(seed.to_owned())));
        if let Some(schema) = config.value.get("schema").and_then(ConfigValue::as_str) {
            let path = config.join(schema);
            let pattern = if path.ends_with(".prisma") {
                path
            } else {
                format!("{}/**/*.prisma", path.trim_end_matches('/'))
            };
            out.extend(generator_inputs(cx, std::slice::from_ref(&pattern)));
            out.push(PluginInput::Entry(pattern));
        }
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let patterns: Vec<String> = SPEC.entry.iter().map(|p| (*p).to_owned()).collect();
        generator_inputs(cx, &patterns)
    }
}

/// What the `generator` providers of the schema files matching `patterns` reference.
fn generator_inputs(cx: &PluginContext<'_>, patterns: &[String]) -> Vec<PluginInput> {
    let mut out = Vec::new();
    for file in find_files(cx.fs, cx.dir, patterns) {
        let Ok(text) = cx.fs.read_to_string(&cx.dir.join(&file)) else {
            continue;
        };
        out.extend(
            generator_providers(&text)
                .into_iter()
                .filter_map(|p| provider_input(&file, p)),
        );
    }
    out
}

/// The literal `provider` of every `generator` block of a schema, also of a block on one line
/// (`generator x { provider = "y" }`).
fn generator_providers(text: &str) -> Vec<&str> {
    let mut providers = Vec::new();
    let mut in_generator = false;
    for line in text.lines() {
        let line = &line[..find_unquoted(line, "//").unwrap_or(line.len())];
        let mut field = line.trim();
        if !in_generator {
            let Some(open) = field
                .strip_prefix("generator")
                .filter(|rest| rest.starts_with(char::is_whitespace))
                .and_then(|rest| rest.find('{').map(|at| &rest[at + 1..]))
            else {
                continue;
            };
            in_generator = true;
            field = open.trim();
        }
        if let Some(close) = find_unquoted(field, "}") {
            field = field[..close].trim();
            in_generator = false;
        }
        if let Some(value) = field.strip_prefix("provider").map(str::trim_start)
            && let Some(value) = value.strip_prefix('=').map(str::trim_start)
            && let Some(value) = value.strip_prefix('"')
            && let Some((provider, _)) = value.split_once('"')
        {
            providers.push(provider.trim());
        }
    }
    providers
}

/// Where `needle` first occurs in `line` outside a double-quoted string.
fn find_unquoted(line: &str, needle: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (at, c) in line.char_indices() {
        if quoted {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
        } else if c == '"' {
            quoted = true;
        } else if line[at..].starts_with(needle) {
            return Some(at);
        }
    }
    None
}

/// A provider of the schema `file` as an input: a relative path is an entry next to the schema, a command is a script
/// and a bare name is a package the schema names.
fn provider_input(file: &str, provider: &str) -> Option<PluginInput> {
    let dir = file.rsplit_once('/').map_or("", |(dir, _)| dir);
    if provider.is_empty() || matches!(provider, "prisma-client-js" | "prisma-client") {
        return None;
    }
    if provider.contains(char::is_whitespace) {
        return Some(PluginInput::Script(provider.to_owned()));
    }
    if is_relative(provider) {
        return Some(PluginInput::Entry(join_rel(dir, provider)));
    }
    package_name(provider).map(|name| PluginInput::FileDependency {
        file: file.to_owned(),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_of_single_line_blocks_and_values_with_slashes() {
        let schema = r#"generator one { provider = "single-line" }
generator two {
  provider = "node ./gen//index.js" // a comment
  output   = "../out"
}
datasource db { provider = "postgresql" }
// generator old { provider = "commented-out" }
generator three { output = "x" }
"#;
        assert_eq!(generator_providers(schema), ["single-line", "node ./gen//index.js"]);
    }
}
