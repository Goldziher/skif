//! Serverless Framework: the handler file of every function, local plugins and plugin packages, and the esbuild
//! `inject` files of `serverless.yml`. A `${file(./x.yml)}` or `${file(./x.yml):key}` variable is read from that file.
//! In a handler, a `${self:a.b}` variable is replaced by the string the config holds there, and any other variable
//! (`${opt:stage}`, `${env:DIR}`), known only when the CLI runs, by a `*` wildcard; a handler may be a glob.

use std::borrow::Cow;

use skif_js_syntax::ConfigValue;

use crate::config_load::load_config;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, is_relative, package_name, strings, values_of};

/// The `serverless-framework` plugin.
pub struct ServerlessFramework;

static SPEC: PluginSpec = PluginSpec {
    id: "serverless-framework",
    enablers: &[Enabler::Dependency("serverless")],
    config: &["serverless.{js,cjs,mjs,ts,cts,mts,yml,yaml}"],
    binaries: &["serverless", "sls"],
    ..PluginSpec::DEFAULT
};

const SOURCE_EXTENSIONS: &[&str] = &["js", "cjs", "mjs", "ts", "cts", "mts", "jsx", "tsx"];

impl Plugin for ServerlessFramework {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let mut out = Vec::new();
        if let Some(functions) = value.get("functions") {
            let functions = file_variable(functions, config, cx);
            for group in values_or_self(&functions) {
                let group = file_variable(group, config, cx);
                let ConfigValue::Object(entries) = group.as_ref() else {
                    continue;
                };
                for (_, function) in entries {
                    let function = file_variable(function, config, cx);
                    if let Some(handler) = function.get("handler").and_then(ConfigValue::as_str) {
                        out.extend(handler_entry(&expand_variables(handler, value, 0), config));
                    }
                }
            }
        }
        if let Some(plugins) = value.get("plugins") {
            for plugin in strings(&file_variable(plugins, config, cx)) {
                if is_relative(plugin) {
                    out.push(PluginInput::ProductionEntry(config.join(plugin)));
                } else {
                    out.extend(package_name(plugin).map(PluginInput::Dependency));
                }
            }
        }
        for path in [["custom", "esbuild"], ["build", "esbuild"]] {
            let Some(esbuild) = get_in(value, &path) else {
                continue;
            };
            if matches!(esbuild, ConfigValue::Bool(false) | ConfigValue::Null) {
                continue;
            }
            out.push(PluginInput::OptionalDependency("esbuild".to_owned()));
            for inject in esbuild.get("inject").map(strings).unwrap_or_default() {
                out.push(PluginInput::ProductionEntry(config.join(inject)));
            }
        }
        out
    }
}

/// The elements of an array, or the value itself.
fn values_or_self(value: &ConfigValue) -> Vec<&ConfigValue> {
    match value {
        ConfigValue::Array(_) => values_of(value),
        other => vec![other],
    }
}

/// How many `${self:…}` variables may lead to one another before the rest are wildcards.
const MAX_VARIABLE_DEPTH: u8 = 8;

/// `text` with every `${…}` variable replaced: a `${self:a.b}` by the string at `a.b` of the config `root`, itself
/// expanded, and anything else (another source, a missing or non-string value, a nested variable) by `*`.
fn expand_variables(text: &str, root: &ConfigValue, depth: u8) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = closing_brace(after) else {
            out.push('*');
            return out;
        };
        out.push_str(&variable_value(&after[..end], root, depth));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The byte index of the `}` that closes a variable whose text starts `text`.
fn closing_brace(text: &str) -> Option<usize> {
    let mut depth = 0_u32;
    for (index, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(index),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The value of the variable `inner` (the text between `${` and `}`); see [`expand_variables`].
fn variable_value(inner: &str, root: &ConfigValue, depth: u8) -> String {
    let reference = inner.split(',').next().unwrap_or_default().trim();
    let resolved = reference
        .strip_prefix("self:")
        .filter(|path| depth < MAX_VARIABLE_DEPTH && !path.is_empty() && !path.contains(['$', '{']))
        .and_then(|path| get_in(root, &path.split('.').collect::<Vec<_>>()))
        .and_then(ConfigValue::as_str)
        .map(|value| expand_variables(value, root, depth + 1));
    resolved.unwrap_or_else(|| "*".to_owned())
}

/// The production entry of a `handler` (`src/handler.main`): the module before the last `.`, with a source extension
/// when it has none. Variables are already expanded; a glob stays a glob.
fn handler_entry(handler: &str, config: &ConfigAst<'_>) -> Option<PluginInput> {
    if handler.contains(['$', '`']) {
        return None;
    }
    let (file, _) = handler.rsplit_once('.')?;
    let has_extension = file
        .rsplit_once('.')
        .is_some_and(|(_, extension)| SOURCE_EXTENSIONS.contains(&extension));
    let file = if has_extension {
        file.to_owned()
    } else {
        format!("{file}.{{js,ts}}")
    };
    Some(PluginInput::ProductionEntry(config.join(&file)))
}

/// The value a `${file(./x.yml)}` or `${file(./x.yml):a.b}` variable stands for, read from the file; any other value
/// as it is. An unreadable file or a missing key is null.
fn file_variable<'v>(value: &'v ConfigValue, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Cow<'v, ConfigValue> {
    let Some(variable) = value.as_str().and_then(|text| text.strip_prefix("${file(")) else {
        return Cow::Borrowed(value);
    };
    let Some((path, rest)) = variable.split_once(')') else {
        return Cow::Borrowed(value);
    };
    let path = config.join(path.trim());
    let selector = rest
        .strip_prefix(':')
        .and_then(|rest| rest.split(['}', ',', ' ']).next())
        .filter(|selector| !selector.is_empty());
    let loaded = cx
        .fs
        .read_to_string(&cx.dir.join(&path))
        .ok()
        .and_then(|source| load_config(&cx.dir.join(&path), &source));
    let selected = loaded.and_then(|loaded| match selector {
        Some(selector) => get_in(&loaded, &selector.split('.').collect::<Vec<_>>()).cloned(),
        None => Some(loaded),
    });
    Cow::Owned(selected.unwrap_or(ConfigValue::Null))
}
