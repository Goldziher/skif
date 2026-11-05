//! Vitest: test files, environments, reporters, coverage providers and setup files named in `test`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::vite;
use crate::support::{
    alias_inputs, entry_glob, get_in, module_input, name_of, objects, package_name, project_files, strings,
};

/// The `vitest` plugin.
pub struct Vitest;

static SPEC: PluginSpec = PluginSpec {
    id: "vitest",
    enablers: &[Enabler::Dependency("vitest"), Enabler::Dependency("vite-plus")],
    config: &[
        "vitest.config.{js,mjs,ts,cjs,mts,cts}",
        "vitest.{workspace,projects}.{js,mjs,ts,cjs,mts,cts,json}",
        "vite.config.{js,mjs,ts,cjs,mts,cts}",
    ],
    entry: &[
        "**/*.{bench,test,test-d,spec,spec-d}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "**/__mocks__/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
    ],
    binaries: &["vitest"],
    ..PluginSpec::DEFAULT
};

const BUILTIN_REPORTERS: &[&str] = &[
    "default",
    "verbose",
    "dot",
    "json",
    "tap",
    "tap-flat",
    "junit",
    "hanging-process",
    "github-actions",
    "basic",
    "tree",
    "blob",
];

impl Plugin for Vitest {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = vite::html_and_build_inputs(config, cx);
        collect(config.value, config, cx, &mut out, 0);
        out
    }
}

/// Handles a config object, a workspace array of them, and the `test.workspace` / `test.projects` they nest.
fn collect(value: &ConfigValue, config: &ConfigAst<'_>, cx: &PluginContext<'_>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 8 {
        return;
    }
    for object in objects(value) {
        for path in [["resolve", "alias"], ["test", "alias"]] {
            if let Some(alias) = get_in(object, &path) {
                out.extend(alias_inputs(alias, config));
            }
        }
        let Some(test) = object.get("test") else { continue };
        test_options(test, config, out);
        for key in ["workspace", "projects"] {
            if let Some(projects) = test.get(key) {
                collect(projects, config, cx, out, depth + 1);
                string_projects(projects, config, cx, out);
            }
        }
    }
}

const PROJECT_CONFIGS: &str = "{vitest,vite}.config.{js,mjs,ts,cjs,mts,cts}";

/// `projects: ["packages/*"]`: each string is a config file, or a directory (or glob) holding a `vitest.config.*` or
/// `vite.config.*`.
fn string_projects(projects: &ConfigValue, config: &ConfigAst<'_>, cx: &PluginContext<'_>, out: &mut Vec<PluginInput>) {
    for project in strings(projects) {
        out.extend(
            project_files(cx, config.base, project, PROJECT_CONFIGS)
                .into_iter()
                .map(PluginInput::ConfigFile),
        );
    }
}

fn test_options(test: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    if let Some(environment) = test.get("environment").and_then(ConfigValue::as_str) {
        out.extend(environment_input(config, environment));
    }
    if let Some(reporters) = test.get("reporters") {
        let reporters: Vec<&ConfigValue> = match reporters {
            ConfigValue::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for name in reporters.into_iter().filter_map(name_of) {
            if !BUILTIN_REPORTERS.contains(&name) {
                out.extend(module_input(config.base, name, false));
            }
        }
    }
    if let Some(coverage) = test.get("coverage") {
        let provider = coverage.get("provider").and_then(ConfigValue::as_str);
        let enabled = coverage.get("enabled").and_then(ConfigValue::as_bool) != Some(false);
        if (enabled || provider.is_some()) && provider != Some("custom") {
            out.push(PluginInput::Dependency(format!(
                "@vitest/coverage-{}",
                provider.unwrap_or("v8")
            )));
        }
    }
    for key in ["setupFiles", "globalSetup", "snapshotSerializers"] {
        for specifier in test.get(key).map(strings).unwrap_or_default() {
            out.extend(module_input(config.base, specifier, true));
        }
    }
    for pattern in test.get("include").map(strings).unwrap_or_default() {
        out.extend(entry_glob(config.base, pattern).map(PluginInput::Entry));
    }
}

fn environment_input(config: &ConfigAst<'_>, environment: &str) -> Option<PluginInput> {
    match environment {
        "node" | "" => None,
        "edge-runtime" => Some(PluginInput::Dependency("@edge-runtime/vm".to_string())),
        "jsdom" | "happy-dom" => Some(PluginInput::Dependency(environment.to_string())),
        custom if custom.starts_with('.') || custom.starts_with('/') => module_input(config.base, custom, true),
        custom => package_name(&format!("vitest-environment-{custom}")).map(PluginInput::Dependency),
    }
}
