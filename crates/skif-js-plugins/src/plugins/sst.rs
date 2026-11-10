//! SST: the function handlers of `sst.config.ts`, written as a `handler` property (`new sst.aws.Function("Api",
//! { handler: "src/api.handler" })`) or as the handler of `api.route("GET /", "src/get.handler")`, in the config or in
//! the modules it imports by relative path (`await import("./infra/api")`, `./infra/api.js` for `infra/api.ts`), are
//! production entries. SST 2 stacks, which `sst.config.ts` imports (`app.stack(API)` with `API` from `./stacks/API`),
//! are read the same way, along with the shorthand of SST 2 constructs that names a handler as a plain string: the
//! values of `routes`, `subscribers`, `notifications` and `targets` (`"GET /": "src/list.main"`) and a `function`,
//! `job` or `consumer` string (`{ function: "src/create.main" }`). SST 3 accepts a `function` string as well.

use std::collections::BTreeSet;
use std::path::Path;

use skif_js_syntax::{ConfigValue, ImportKind, ParseOptions, extract_property_values, parse_module};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, join_rel, leaf_strings, parent_dir};

/// The `sst` plugin.
pub struct Sst;

static SPEC: PluginSpec = PluginSpec {
    id: "sst",
    enablers: &[Enabler::Dependency("sst")],
    config: &["sst.config.ts"],
    binaries: &["sst"],
    ..PluginSpec::DEFAULT
};

const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// How many modules the config may import, directly or through other modules, before reading stops.
const MAX_MODULES: usize = 256;

impl Plugin for Sst {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The handlers are read from the source; what the config exports (`$config({ … })`) is not needed.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let config_file = join_rel(config.base, config.file_name());
        let mut seen = BTreeSet::from([config_file.clone()]);
        let mut queue = vec![(config_file, config.source.to_owned())];
        let mut handlers = BTreeSet::new();
        while let Some((file, source)) = queue.pop() {
            let path = cx.dir.join(&file);
            for value in extract_property_values(&source, &path, "handler") {
                handlers.extend(leaf_strings(&value).into_iter().map(str::to_owned));
            }
            handlers.extend(shorthand_handlers(&source, &path));
            handlers.extend(route_handlers(&source));
            let options = ParseOptions {
                in_file_references: false,
                member_accesses: false,
            };
            let Ok(module) = parse_module(&path, &source, &options) else {
                continue;
            };
            for import in module.imports {
                if import.kind == ImportKind::TypeOnly || !is_relative(&import.specifier) {
                    continue;
                }
                if seen.len() >= MAX_MODULES {
                    break;
                }
                let target = join_rel(parent_dir(&file), &import.specifier);
                if let Some((imported, text)) = read_module(cx, &target, &seen) {
                    seen.insert(imported.clone());
                    queue.push((imported, text));
                }
            }
        }
        handlers
            .iter()
            .filter_map(|handler| handler_entry(handler, config.base))
            .collect()
    }
}

/// The file a relative import names, tried as TypeScript resolves it: a `.js`, `.mjs` or `.cjs` specifier as its
/// TypeScript source (`./api.js` is `api.ts` or `api.tsx`), as written, with a source extension and as a directory
/// index. `None` when the file is not found or is already in `seen`, which is checked before reading.
fn read_module(cx: &PluginContext<'_>, target: &str, seen: &BTreeSet<String>) -> Option<(String, String)> {
    let typescript: &[&str] = match target.rsplit_once('.') {
        Some((_, "js")) => &["ts", "tsx"],
        Some((_, "mjs")) => &["mts"],
        Some((_, "cjs")) => &["cts"],
        _ => &[],
    };
    let stem = target.rsplit_once('.').map_or(target, |(stem, _)| stem);
    let candidates = typescript
        .iter()
        .map(|extension| format!("{stem}.{extension}"))
        .chain(std::iter::once(target.to_owned()))
        .chain(EXTENSIONS.iter().map(|extension| format!("{target}.{extension}")))
        .chain(EXTENSIONS.iter().map(|extension| format!("{target}/index.{extension}")));
    for candidate in candidates {
        let is_source = candidate
            .rsplit_once('.')
            .is_some_and(|(_, extension)| EXTENSIONS.contains(&extension));
        if !is_source {
            continue;
        }
        if seen.contains(&candidate) {
            return None;
        }
        if let Ok(text) = cx.fs.read_to_string(&cx.dir.join(&candidate)) {
            return Some((candidate, text.to_string()));
        }
    }
    None
}

/// The production entry of a handler (`packages/functions/src/api.handler`): the module before the last `.`, relative
/// to the directory of `sst.config.ts`, with any source extension.
fn handler_entry(handler: &str, base: &str) -> Option<PluginInput> {
    let (module, _) = handler.rsplit_once('.')?;
    if module.is_empty() || module.contains(['$', '`', '*']) {
        return None;
    }
    let has_extension = module
        .rsplit_once('.')
        .is_some_and(|(_, extension)| EXTENSIONS.contains(&extension));
    let file = if has_extension {
        module.to_owned()
    } else {
        format!("{module}.{{{}}}", EXTENSIONS.join(","))
    };
    Some(PluginInput::ProductionEntry(join_rel(base, &file)))
}

/// Keys of SST 2 constructs whose object maps names to functions, each written as a handler string or an object.
const FUNCTION_MAPS: &[&str] = &["routes", "subscribers", "notifications", "targets"];

/// Keys whose string value is a handler (`job: "src/cron.handler"`); an object value holds a `handler` or `function`.
const FUNCTION_KEYS: &[&str] = &["function", "job", "consumer"];

/// The handlers written in the string shorthand of SST constructs: the string values of a [`FUNCTION_MAPS`] object
/// and a string under one of [`FUNCTION_KEYS`]. An object value is not read here: its `handler` or `function` is
/// found on its own.
fn shorthand_handlers(source: &str, path: &Path) -> Vec<String> {
    let maps = FUNCTION_MAPS.iter().flat_map(|key| {
        extract_property_values(source, path, key)
            .into_iter()
            .flat_map(|value| match value {
                ConfigValue::Object(entries) => entries
                    .into_iter()
                    .filter_map(|(_, value)| value.as_str().map(str::to_owned))
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
    });
    let keys = FUNCTION_KEYS.iter().flat_map(|key| {
        extract_property_values(source, path, key)
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
    });
    maps.chain(keys).collect()
}

/// The string literal handlers of `.route(<route>, "<handler>")` calls, read from the text.
fn route_handlers(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find(".route(") {
        rest = &rest[at + ".route(".len()..];
        let Some((_, after_route)) = string_literal(rest) else {
            continue;
        };
        let Some(after_comma) = after_route.trim_start().strip_prefix(',') else {
            continue;
        };
        if let Some((handler, _)) = string_literal(after_comma) {
            found.push(handler.to_owned());
        }
    }
    found
}

/// The string literal at the start of `text` (after whitespace) and the text after it; `None` for a template literal
/// with substitutions or anything else.
fn string_literal(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let quote = text.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let body = &text[1..];
    let end = body.find(quote)?;
    let value = &body[..end];
    let computed = value.contains('\\') || (quote == '`' && value.contains("${"));
    (!computed).then(|| (value, &body[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_handlers_are_the_second_string_argument() {
        let source =
            r#"api.route("GET /", "src/get.handler"); api.route(`POST /x`, { handler: "y" }); a.route(x, "z.h")"#;
        assert_eq!(route_handlers(source), ["src/get.handler"]);
    }

    #[test]
    fn handlers_name_a_module_and_an_export() {
        assert_eq!(
            handler_entry("src/api.handler", ""),
            Some(PluginInput::ProductionEntry(
                "src/api.{ts,tsx,mts,cts,js,jsx,mjs,cjs}".to_owned()
            ))
        );
        assert_eq!(
            handler_entry("./src/api.ts.main", "infra"),
            Some(PluginInput::ProductionEntry("infra/src/api.ts".to_owned()))
        );
        assert_eq!(handler_entry("index", ""), None);
    }
}
