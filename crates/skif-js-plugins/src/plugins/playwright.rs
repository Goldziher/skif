//! Playwright: test files, global setup, reporters and the web server command named in the config.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, join_rel, module_input, name_of, objects, strings};

/// The `playwright` plugin.
pub struct Playwright;

const TEST_FILES: &str = "**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}";

static SPEC: PluginSpec = PluginSpec {
    id: "playwright",
    enablers: &[Enabler::Dependency("@playwright/test")],
    config: &["playwright.config.{js,cjs,mjs,ts,cts,mts}"],
    entry: &[TEST_FILES],
    binaries: &["playwright"],
    ..PluginSpec::DEFAULT
};

const BUILTIN_REPORTERS: &[&str] = &["dot", "line", "list", "junit", "html", "blob", "json", "github", "null"];

impl Plugin for Playwright {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let root_dir = config.value.get("testDir").and_then(ConfigValue::as_str);
        let mut projects = vec![config.value];
        if let Some(listed) = config.value.get("projects") {
            projects.extend(objects(listed));
        }
        for project in projects {
            test_files(project, root_dir, config, &mut out);
        }
        for key in ["globalSetup", "globalTeardown"] {
            for path in config.value.get(key).map(strings).unwrap_or_default() {
                out.push(PluginInput::Entry(config.join(path)));
            }
        }
        if let Some(reporter) = config.value.get("reporter") {
            let reporters: Vec<&ConfigValue> = match reporter {
                ConfigValue::Array(items) => items.iter().collect(),
                other => vec![other],
            };
            for name in reporters.into_iter().filter_map(name_of) {
                if !BUILTIN_REPORTERS.contains(&name) {
                    out.extend(module_input(config.base, name, true));
                }
            }
        }
        if let Some(servers) = config.value.get("webServer") {
            for server in objects(servers) {
                if let Some(command) = server.get("command").and_then(ConfigValue::as_str) {
                    out.push(PluginInput::Script(command.to_string()));
                }
            }
        }
        out
    }
}

/// Test globs of one project: `testMatch` under its `testDir`, or every test file under a custom `testDir`.
fn test_files(project: &ConfigValue, root_dir: Option<&str>, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    let dir = project.get("testDir").and_then(ConfigValue::as_str).or(root_dir);
    let base = dir.map_or_else(|| config.base.to_string(), |d| join_rel(config.base, d));
    let patterns = project.get("testMatch").map(strings).unwrap_or_default();
    if !patterns.is_empty() {
        for pattern in patterns {
            out.extend(entry_glob(&base, pattern).map(PluginInput::Entry));
        }
    } else if dir.is_some() {
        out.push(PluginInput::Entry(join_rel(&base, TEST_FILES)));
    }
}
