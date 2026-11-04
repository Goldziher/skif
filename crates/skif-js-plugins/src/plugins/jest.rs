//! Jest: presets, runners, environments, reporters, setup files and transforms named in the config.

use skif_js_syntax::ConfigValue;

use crate::config_load::from_json;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::babel;
use crate::support::{
    conventional, entry_glob, is_relative, module_input, name_of, objects, package_name, project_files, strings,
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
        "**/__mocks__/**/*.{js,ts,jsx,tsx}",
    ],
    package_json_key: Some("jest"),
    binaries: &["jest"],
    ..PluginSpec::DEFAULT
};

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
}

/// Dependency by convention (`runner: "eslint"` is `jest-runner-eslint`), or the path when relative.
fn prefixed(config: &ConfigAst<'_>, name: &str, namespace: &str) -> Option<PluginInput> {
    if is_relative(name) || name.starts_with("<rootDir>") {
        return module_input(config.base, name, false);
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
        for specifier in object.get(key).map(strings).unwrap_or_default() {
            out.extend(module_input(base, specifier, false));
        }
    }
    if let Some(ConfigValue::Object(transforms)) = object.get("transform") {
        for (_, transformer) in transforms {
            transform(transformer, config, out);
        }
    }
    if let Some(ConfigValue::Object(mappers)) = object.get("moduleNameMapper") {
        for (_, mapper) in mappers {
            for target in strings(mapper) {
                if !target.contains('$') {
                    out.extend(module_input(base, target, false));
                }
            }
        }
    }
    for pattern in object.get("testMatch").map(strings).unwrap_or_default() {
        out.extend(entry_glob(base, pattern).map(PluginInput::Entry));
    }
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
