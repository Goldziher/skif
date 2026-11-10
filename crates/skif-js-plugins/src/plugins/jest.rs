//! Jest: presets, runners, environments, reporters, setup files and transforms named in the config, and the test
//! files `testMatch`, `testRegex` and `testPathIgnorePatterns` select.

use skif_js_syntax::ConfigValue;

use crate::config_load::from_json;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::babel;
use crate::support::{
    conventional, entry_glob, entry_strings, is_relative, key_unknown, module_input, name_of, objects, package_name,
    pattern_alias, project_files, shallow_key_unknown, strings,
};

/// The `jest` plugin.
pub struct Jest;

static SPEC: PluginSpec = PluginSpec {
    id: "jest",
    enablers: &[Enabler::Dependency("jest"), Enabler::ManifestNamePrefix("jest-presets")],
    config: &["jest.config.{js,ts,mjs,mts,cjs,cts,json}"],
    entry: &[
        "**/__tests__/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        // Jest's `**/?(*.)+(spec|test).[jt]s?(x)` also matches a file named just `test.js` or `spec.ts`.
        "**/{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
    ],
    package_json_key: Some("jest"),
    binaries: &["jest"],
    ..PluginSpec::DEFAULT
};

/// The manual mocks Jest loads by convention, whatever selects the test files and whichever of them it ignores; an
/// entry of every package with the plugin, not a default test glob.
const MOCKS: &str = "**/__mocks__/**/*.{js,ts,jsx,tsx}";

const BUILTIN_REPORTERS: &[&str] = &["default", "github-actions", "summary"];

impl Plugin for Jest {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        collect(config.value, config, cx, &mut out, 0);
        out
    }

    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vec![PluginInput::Entry(MOCKS.to_owned())]
    }
}

/// Dependency by convention (`runner: "eslint"` is `jest-runner-eslint`), the path when relative, and the name itself
/// when it is already a jest package (`runner: "jest-light-runner"`): jest falls back to the bare name.
fn prefixed(config: &ConfigAst<'_>, name: &str, namespace: &str) -> Option<PluginInput> {
    if is_relative(name) || name.starts_with("<rootDir>") {
        return module_input(config.base, name, false);
    }
    if name.starts_with("jest-") {
        return package_name(name).map(PluginInput::Dependency);
    }
    Some(PluginInput::Dependency(conventional(name, namespace)))
}

const PROJECT_CONFIGS: &str = "jest.config.{js,ts,mjs,mts,cjs,cts,json}";

fn collect(value: &ConfigValue, config: &ConfigAst<'_>, cx: &PluginContext<'_>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 4 {
        return;
    }
    for object in objects(value) {
        collect_object(object, config, out);
        test_paths(object, config, depth, out);
        if shallow_key_unknown(object, "projects") {
            out.push(PluginInput::EntriesIncomplete);
        }
        if let Some(projects) = object.get("projects") {
            collect(projects, config, cx, out, depth + 1);
            string_projects(projects, config, cx, out, depth);
        }
    }
}

/// `projects: ["<rootDir>/packages/*"]`: each string is a directory (or glob) with its own `jest.config.*` or a
/// `package.json#jest`, or a config file.
fn string_projects(
    projects: &ConfigValue,
    config: &ConfigAst<'_>,
    cx: &PluginContext<'_>,
    out: &mut Vec<PluginInput>,
    depth: u8,
) {
    for project in strings(projects) {
        out.extend(
            project_files(cx, config.base, project, PROJECT_CONFIGS)
                .into_iter()
                .map(PluginInput::ConfigFile),
        );
        for manifest in project_files(cx, config.base, project, "package.json") {
            let Ok(text) = cx.fs.read_to_string(&cx.dir.join(&manifest)) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(jest) = json.get("jest").map(from_json) else {
                continue;
            };
            let path = cx.dir.join(&manifest);
            if path == config.path {
                continue;
            }
            let base = manifest.rsplit_once('/').map_or("", |(parent, _)| parent);
            let project_config = ConfigAst {
                path: &path,
                base,
                value: &jest,
                source: "",
            };
            collect(&jest, &project_config, cx, out, depth + 1);
        }
    }
}

fn collect_object(object: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    let base = config.base;
    let string = |key: &str| object.get(key).and_then(ConfigValue::as_str);

    if let Some(preset) = string("preset") {
        if is_relative(preset) {
            out.extend(module_input(base, preset, false));
        } else {
            out.extend(package_name(preset).map(PluginInput::Dependency));
        }
    }
    if let Some(runner) = object.get("runner").and_then(name_of) {
        out.extend(prefixed(config, runner, "jest-runner"));
    }
    for key in ["runtime", "resolver", "testResultsProcessor", "snapshotResolver"] {
        if let Some(name) = string(key) {
            out.extend(module_input(base, name, false));
        }
    }
    if let Some(runner) = string("testRunner")
        && !runner.starts_with("jest-circus")
    {
        out.extend(module_input(base, runner, false));
    }
    if let Some(environment) = string("testEnvironment") {
        out.extend(environment_input(config, environment));
    }
    if let Some(reporters) = object.get("reporters").and_then(ConfigValue::as_array) {
        for name in reporters.iter().filter_map(name_of) {
            if !BUILTIN_REPORTERS.contains(&name) {
                out.extend(module_input(base, name, false));
            }
        }
    }
    if let Some(plugins) = object.get("watchPlugins").and_then(ConfigValue::as_array) {
        for name in plugins.iter().filter_map(name_of) {
            out.extend(prefixed(config, name, "jest-watch"));
        }
    }
    if let Some(sequencer) = string("testSequencer") {
        out.extend(prefixed(config, sequencer, "jest-sequencer"));
    }
    for key in [
        "setupFiles",
        "setupFilesAfterEnv",
        "globalSetup",
        "globalTeardown",
        "snapshotSerializers",
    ] {
        for specifier in entry_strings(object, key, out) {
            out.extend(module_input(base, specifier, false));
        }
    }
    if let Some(ConfigValue::Object(transforms)) = object.get("transform") {
        for (_, transformer) in transforms {
            transform(transformer, config, out);
        }
    }
    if let Some(ConfigValue::Object(mappers)) = object.get("moduleNameMapper") {
        for (key, mapper) in mappers {
            if let ConfigValue::String(replacement) = mapper {
                let replacement = ConfigValue::String(replacement.replace("<rootDir>", "."));
                out.extend(pattern_alias(key, &replacement, config));
            }
            for target in strings(mapper) {
                if !target.contains('$') {
                    out.extend(module_input(base, target, false));
                }
            }
        }
    }
    for pattern in entry_strings(object, "testMatch", out) {
        out.extend(entry_glob(base, pattern).map(PluginInput::TestEntry));
    }
}

/// `testRegex` and `testPathIgnorePatterns`: regular expressions Jest matches against the absolute path of a file.
///
/// A `testRegex` is an entry regex, and it replaces the default test globs when it is the only way the package's
/// own config selects tests (the config in the package directory itself, without `projects`); otherwise the defaults
/// stay, which can only keep more files used. Every `testPathIgnorePatterns` regex keeps the files it matches out of
/// Jest's entries, and an element that is not known statically is skipped, which also keeps more files used.
fn test_paths(object: &ConfigValue, config: &ConfigAst<'_>, depth: u8, out: &mut Vec<PluginInput>) {
    let base = config.base;
    let regexes = if key_unknown(object, "testRegex") {
        out.push(PluginInput::EntriesIncomplete);
        Vec::new()
    } else {
        regex_sources(object.get("testRegex"))
    };
    if !regexes.is_empty() && depth == 0 && base.is_empty() && object.get("projects").is_none() {
        out.push(PluginInput::ReplaceDefaultEntries);
    }
    out.extend(
        regexes
            .into_iter()
            .map(|regex| PluginInput::EntryRegex(root_dir_regex(base, regex))),
    );
    out.extend(
        regex_sources(object.get("testPathIgnorePatterns"))
            .into_iter()
            .map(|regex| PluginInput::EntryIgnoreRegex(root_dir_regex(base, regex))),
    );
}

/// The regular expressions of a string, a regex literal or an array of them.
fn regex_sources(value: Option<&ConfigValue>) -> Vec<&str> {
    let items: Vec<&ConfigValue> = match value {
        Some(ConfigValue::Array(items)) => items.iter().collect(),
        Some(other) => vec![other],
        None => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| match item {
            ConfigValue::String(source) | ConfigValue::Regex(source) => Some(source.as_str()),
            _ => None,
        })
        .filter(|source| !source.is_empty())
        .collect()
}

/// A Jest path regex for a match against `/` and the path relative to the workspace directory: `<rootDir>` stands
/// for the directory of the config (`base`), and a leading one anchors the regex there, so `<rootDir>/e2e/` matches
/// `/e2e/x.test.ts` but not `/lib/e2e/x.test.ts`.
fn root_dir_regex(base: &str, regex: &str) -> String {
    let dir = if base.is_empty() {
        String::new()
    } else {
        format!("/{}", escape_regex(base))
    };
    match regex.strip_prefix("<rootDir>") {
        Some(rest) => format!("^{dir}{}", rest.replace("<rootDir>", &dir)),
        None => regex.replace("<rootDir>", &dir),
    }
}

/// `text` with the characters a regular expression gives a meaning escaped.
fn escape_regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn environment_input(config: &ConfigAst<'_>, environment: &str) -> Option<PluginInput> {
    match environment {
        "node" => Some(PluginInput::OptionalDependency("jest-environment-node".to_string())),
        "jsdom" => Some(PluginInput::Dependency("jest-environment-jsdom".to_string())),
        other => prefixed(config, other, "jest-environment"),
    }
}

fn transform(transformer: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    let Some(name) = name_of(transformer) else { return };
    out.extend(module_input(config.base, name, false));
    if name == "babel-jest"
        && let Some(options) = transformer.as_array().and_then(|items| items.get(1))
    {
        out.extend(babel::dependencies(options, config.base, 0));
    }
}
