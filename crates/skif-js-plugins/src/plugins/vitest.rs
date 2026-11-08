//! Vitest: test files, environments, reporters, coverage providers and setup files named in `test`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{
    ConfigAst, Enabler, IncompleteCause, IncompleteScope, Plugin, PluginContext, PluginInput, PluginSpec,
};
use crate::plugins::vite;
use crate::support::{
    alias_inputs, config_unknown, convention_scope, entry_glob, entry_strings, get_in, module_input, name_of, objects,
    own_path_unknown, package_name, project_files, shallow_key_unknown, strings,
};

/// Where test runners find what a config names besides the test files of [`SPEC`]: test and fixture directories, and
/// setup and teardown modules. Shared with the Playwright plugin.
pub(crate) const TEST_CONVENTIONS: &[&str] = &[
    "**/{test,tests,__tests__,__mocks__,__fixtures__,e2e,spec,specs,fixtures,mocks,testing,test-utils}/**",
    "**/*{setup,Setup,teardown,Teardown}*.{ext}",
    "**/*.{test,spec,bench,e2e,integration}*.{ext}",
];

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

    /// What a spread or merged base test config hides is tests: test files, test directories and setup modules. A
    /// file key the config computes itself, or a config unknown as a whole (a factory called with this package's
    /// options), may name any file.
    fn incomplete_scope(&self, _cause: IncompleteCause, config: &ConfigAst<'_>) -> IncompleteScope {
        if computes_files(config.value, 0) {
            return IncompleteScope::Package;
        }
        convention_scope(&SPEC, config, TEST_CONVENTIONS)
    }
}

/// The keys a Vitest config names files by: the project root holding `index.html`, the test globs and the setup
/// modules.
const FILE_KEYS: &[&[&str]] = &[
    &["root"],
    &["test", "include"],
    &["test", "setupFiles"],
    &["test", "globalSetup"],
];

/// Whether a config, a workspace list of them or a project they nest is unknown as a whole or computes one of
/// [`FILE_KEYS`] in its own text, leaving aside what an unknown spread or merge may set.
fn computes_files(value: &ConfigValue, depth: u8) -> bool {
    if depth > 8 {
        return false;
    }
    if depth == 0 && config_unknown(value) {
        return true;
    }
    objects(value).into_iter().any(|object| {
        FILE_KEYS.iter().any(|path| own_path_unknown(object, path))
            || ["workspace", "projects"]
                .iter()
                .filter_map(|key| get_in(object, &["test", key]))
                .any(|projects| computes_files(projects, depth + 1))
    })
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
        if shallow_key_unknown(object, "test") {
            out.push(PluginInput::EntriesIncomplete);
        }
        let Some(test) = object.get("test") else { continue };
        test_options(test, config, out);
        for key in ["workspace", "projects"] {
            if shallow_key_unknown(test, key) {
                out.push(PluginInput::EntriesIncomplete);
            }
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
        for specifier in entry_strings(test, key, out) {
            out.extend(module_input(config.base, specifier, true));
        }
    }
    for pattern in entry_strings(test, "include", out) {
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
