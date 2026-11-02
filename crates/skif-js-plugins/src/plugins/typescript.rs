//! TypeScript: `tsconfig.json` chains and the packages its options name.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, join_rel, objects, package_name, strip_wildcard, strings};

/// The `typescript` plugin.
pub struct Typescript;

static SPEC: PluginSpec = PluginSpec {
    id: "typescript",
    enablers: &[
        Enabler::Dependency("typescript"),
        Enabler::Dependency("@typescript/native"),
        Enabler::Dependency("@typescript/native-preview"),
    ],
    config: &["tsconfig.json", "tsconfig.*.json"],
    binaries: &["tsc", "tsserver", "tsgo"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Typescript {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let mut out = Vec::new();
        if let Some(extends) = value.get("extends") {
            for specifier in strings(extends) {
                if is_relative(specifier) {
                    out.push(PluginInput::ConfigFile(config.join(&with_json_extension(specifier))));
                } else if let Some(name) = package_name(specifier) {
                    out.push(PluginInput::Dependency(name));
                }
            }
        }
        if let Some(ConfigValue::Array(references)) = value.get("references") {
            for path in references
                .iter()
                .filter_map(|r| r.get("path"))
                .filter_map(ConfigValue::as_str)
            {
                let file = if std::path::Path::new(path)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                {
                    path.to_string()
                } else {
                    format!("{}/tsconfig.json", path.trim_end_matches('/'))
                };
                out.push(PluginInput::ConfigFile(config.join(&file)));
            }
        }
        let Some(options) = value.get("compilerOptions") else {
            return out;
        };
        if let Some(types) = options.get("types") {
            for name in strings(types) {
                if let Some(package) = type_package(name, cx) {
                    out.push(PluginInput::Dependency(package));
                }
            }
        }
        if let Some(source) = options.get("jsxImportSource").and_then(ConfigValue::as_str)
            && let Some(name) = package_name(source)
        {
            out.push(PluginInput::Dependency(name));
        }
        if let Some(plugins) = options.get("plugins") {
            for plugin in objects(plugins) {
                if let Some(name) = plugin.get("name").and_then(ConfigValue::as_str).and_then(package_name) {
                    out.push(PluginInput::Dependency(name));
                }
            }
        }
        out.extend(paths(options, config));
        if let (Some(out_dir), Some(root_dir)) = (
            options.get("outDir").and_then(ConfigValue::as_str),
            options.get("rootDir").and_then(ConfigValue::as_str),
        ) {
            out.push(PluginInput::BuildDir {
                out_dir: config.join(out_dir),
                root_dir: config.join(root_dir),
            });
        }
        if let Some(conditions) = options.get("customConditions") {
            out.extend(strings(conditions).into_iter().map(|name| PluginInput::Condition(name.to_owned())));
        }
        if options.get("importHelpers").and_then(ConfigValue::as_bool) == Some(true) {
            out.push(PluginInput::Dependency("tslib".to_string()));
        }
        out
    }
}

/// `compilerOptions.paths` as aliases. Targets are relative to `baseUrl`, or to the config file without one.
fn paths(options: &ConfigValue, config: &ConfigAst<'_>) -> Vec<PluginInput> {
    let Some(paths) = options.get("paths").and_then(ConfigValue::as_object) else {
        return Vec::new();
    };
    let root = options.get("baseUrl").and_then(ConfigValue::as_str).unwrap_or(".");
    let mut out = Vec::new();
    for (key, targets) in paths {
        let find = strip_wildcard(key);
        if find.is_empty() {
            continue;
        }
        for target in strings(targets) {
            out.push(PluginInput::Alias {
                find: find.to_owned(),
                target: Some(config.join(&join_rel(root, strip_wildcard(target)))),
            });
        }
    }
    out
}

fn with_json_extension(path: &str) -> String {
    if std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        path.to_string()
    } else {
        format!("{path}.json")
    }
}

/// `types: ["node"]` names `@types/node` unless a package called `node` is a dependency; `vitest/globals` names `vitest`.
fn type_package(name: &str, cx: &PluginContext<'_>) -> Option<String> {
    let package = package_name(name)?;
    let has_subpath = name.matches('/').count() > usize::from(name.starts_with('@'));
    if has_subpath || cx.has_dependency(&package) {
        return Some(package);
    }
    let types = match package.strip_prefix('@') {
        Some(scoped) => format!("@types/{}", scoped.replace('/', "__")),
        None => format!("@types/{package}"),
    };
    Some(types)
}
