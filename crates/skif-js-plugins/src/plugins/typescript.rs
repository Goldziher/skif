//! TypeScript: `tsconfig.json` chains and the packages its options name.

use skif_js_syntax::{ConfigValue, is_declaration_file};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{is_relative, join_rel, objects, package_name, strings, strip_wildcard};

/// The `typescript` plugin.
pub struct Typescript;

/// The plugin's identifier.
pub const ID: crate::plugin::PluginId = "typescript";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[
        Enabler::Dependency("typescript"),
        Enabler::Dependency("@typescript/native"),
        Enabler::Dependency("@typescript/native-preview"),
        Enabler::File("jsconfig.json"),
    ],
    config: &["tsconfig.json", "tsconfig.*.json", "jsconfig.json"],
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
        // `types: ["node"]` loads the types of `node`, which `@types/node` provides as well; `vitest/globals` those of
        // `vitest`. TypeScript 6 reads `*` as every package of the type roots, which names none.
        if let Some(types) = options.get("types") {
            let roots = local_type_roots(options, config);
            out.extend(
                strings(types)
                    .into_iter()
                    .filter(|name| name.trim() != "*" && !in_type_root(cx, &roots, name))
                    .filter_map(package_name)
                    .map(|name| compiler_dependency(name, true)),
            );
        }
        if let Some(source) = options.get("jsxImportSource").and_then(ConfigValue::as_str)
            && let Some(name) = package_name(source)
        {
            out.push(compiler_dependency(name, false));
        }
        // `jsx: "react-jsx"` does not make `react` a dependency here: a shared base config sets it for packages that
        // do not use React, and the module graph credits `react` where a file contains JSX.
        if let Some(plugins) = options.get("plugins") {
            for plugin in objects(plugins) {
                if let Some(name) = plugin.get("name").and_then(ConfigValue::as_str).and_then(package_name) {
                    out.push(compiler_dependency(name, false));
                }
            }
        }
        out.extend(paths(options, config));
        if let Some(out_dir) = options.get("outDir").and_then(ConfigValue::as_str) {
            let root_dir = options
                .get("rootDir")
                .and_then(ConfigValue::as_str)
                .map(str::to_owned)
                .or_else(|| inferred_root_dir(value));
            if let Some(root_dir) = root_dir {
                out.push(PluginInput::BuildDir {
                    out_dir: config.join(out_dir),
                    root_dir: config.join(&root_dir),
                });
            }
        }
        if let Some(conditions) = options.get("customConditions") {
            out.extend(
                strings(conditions)
                    .into_iter()
                    .map(|name| PluginInput::Condition(name.to_owned())),
            );
        }
        if options.get("importHelpers").and_then(ConfigValue::as_bool) == Some(true) {
            out.push(compiler_dependency("tslib".to_string(), false));
        }
        out
    }
}

/// The `typeRoots` directories of the project, relative to the workspace: those outside `node_modules`, where
/// TypeScript looks a `types` name up before packages.
fn local_type_roots(options: &ConfigValue, config: &ConfigAst<'_>) -> Vec<String> {
    options
        .get("typeRoots")
        .map(strings)
        .unwrap_or_default()
        .into_iter()
        .filter(|root| !root.split(['/', '\\']).any(|part| part == "node_modules"))
        .map(|root| config.join(root))
        .collect()
}

/// Whether one of `roots` holds the types `name` loads: a directory with an `index.d.ts` or a `package.json`.
fn in_type_root(cx: &PluginContext<'_>, roots: &[String], name: &str) -> bool {
    roots.iter().any(|root| {
        let dir = cx.dir.join(join_rel(root, name));
        cx.fs.is_file(&dir.join("index.d.ts")) || cx.fs.is_file(&dir.join("package.json"))
    })
}

/// `compilerOptions.baseUrl` and `paths`.
///
/// `baseUrl` is a directory bare specifiers are looked up in, after the `paths`. Targets of `paths` are relative to
/// `baseUrl`, or to the config file without one; a key with a prefix is an alias, and the catch-all `*` (whose targets
/// are tried in order) adds lookup directories.
fn paths(options: &ConfigValue, config: &ConfigAst<'_>) -> Vec<PluginInput> {
    let base_url = options.get("baseUrl").and_then(ConfigValue::as_str);
    let mut out = Vec::new();
    let root = base_url.unwrap_or(".");
    for (key, targets) in options
        .get("paths")
        .and_then(ConfigValue::as_object)
        .unwrap_or_default()
    {
        let find = strip_wildcard(key);
        // A wildcard inside the key (`*.svg`) cannot be a prefix alias.
        if find.contains('*') {
            continue;
        }
        for target in strings(targets) {
            let stripped = strip_wildcard(target);
            // A wildcard inside the target (`./icons/*.svg`) cannot be followed as a directory: the alias is known
            // not to be a package, but its target is computed.
            if stripped.contains('*') {
                if !find.is_empty() {
                    out.push(PluginInput::Alias {
                        find: find.to_owned(),
                        target: None,
                    });
                }
                continue;
            }
            let path = config.join(&join_rel(root, stripped));
            out.push(if find.is_empty() {
                PluginInput::ModuleDir(path)
            } else {
                PluginInput::Alias {
                    find: find.to_owned(),
                    target: Some(path),
                }
            });
        }
    }
    out.extend(base_url.map(|base_url| PluginInput::ModuleDir(config.join(base_url))));
    out
}

/// The extensions of the files TypeScript reads as inputs, `json` included for `resolveJsonModule`.
const INPUT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "json"];

/// Whether a path segment names a file TypeScript reads as an input.
fn has_input_extension(segment: &str) -> bool {
    segment.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty() && INPUT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// The `rootDir` TypeScript uses when a config sets none: the longest common directory of its input files, here
/// approximated from the `include` and `files` the config lists. An `include` pattern stands for its directory part
/// before the first wildcard, and a listed file for its directory; declaration files are not inputs and are skipped.
/// Without `include` or `files` the inputs are not known statically, and the `dist` to `src` convention applies.
fn inferred_root_dir(value: &ConfigValue) -> Option<String> {
    let mut roots = Vec::new();
    for pattern in value.get("include").map(strings).unwrap_or_default() {
        if is_declaration_file(pattern) {
            continue;
        }
        let segments: Vec<&str> = pattern.split('/').collect();
        let wildcard = segments
            .iter()
            .position(|segment| segment.contains(['*', '?', '{', '[']));
        let directory = match wildcard {
            Some(at) => &segments[..at],
            // A pattern without a wildcard names a directory, or a file when it has an input extension
            // (`packages/v1.2` is a directory).
            None if segments.last().is_some_and(|last| has_input_extension(last)) => &segments[..segments.len() - 1],
            None => &segments[..],
        };
        roots.push(join_rel("", &directory.join("/")));
    }
    for file in value.get("files").map(strings).unwrap_or_default() {
        if is_declaration_file(file) {
            continue;
        }
        roots.push(join_rel(
            "",
            file.rsplit_once('/').map_or("", |(directory, _)| directory),
        ));
    }
    let (first, rest) = roots.split_first()?;
    let mut common: Vec<&str> = first.split('/').filter(|segment| !segment.is_empty()).collect();
    for root in rest {
        let segments: Vec<&str> = root.split('/').filter(|segment| !segment.is_empty()).collect();
        let shared = common.iter().zip(&segments).take_while(|(a, b)| a == b).count();
        common.truncate(shared);
    }
    Some(if common.is_empty() {
        ".".to_owned()
    } else {
        common.join("/")
    })
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

/// A package a compiler option names; see [`PluginInput::CompilerDependency`].
fn compiler_dependency(name: String, type_only: bool) -> PluginInput {
    PluginInput::CompilerDependency { name, type_only }
}
