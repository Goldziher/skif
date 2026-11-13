//! Rstest: test files and mocks (`include`, under `root`), the test environment, setup files, browser mode, aliases
//! and the coverage provider of `rstest.config.*`, and the configs or directories its `projects` name.
//!
//! A project's `exclude` list (`["src/legacy/**"]` or `{ patterns: [...] }`) and its negated `include` patterns are
//! `!` entries below the project's root, which remove the files they match from the package's entries, so the
//! default test and mock globs, kept as every plugin keeps its conventional entries, cannot bring an excluded file
//! back. A `!` entry applies to every entry of the package, not to its project alone: a file that one project excludes
//! and another project with the same root includes is not an entry. Rstest's own default exclusions
//! (`node_modules`, `dist` and tool directories) are not added, as they would drop those directories from the entries
//! of every plugin, so `{ override }` changes nothing. The modules a test mocks with `rs.mock("x")` are read by the
//! parser with the other test runners' mock calls.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::flag_values;
use crate::support::{
    alias_inputs, entry_glob, entry_pattern, get_in, join_rel, module_input, objects, package_name, project_files,
    strings,
};

/// The `rstest` plugin.
pub struct Rstest;

const TEST_FILES: &str = "**/*.{test,spec}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}";
const MOCKS: &str = "**/__mocks__/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}";
const PROJECT_CONFIGS: &str = "rstest.config.{js,cjs,mjs,ts,cts,mts}";

static SPEC: PluginSpec = PluginSpec {
    id: "rstest",
    enablers: &[Enabler::Dependency("@rstest/core")],
    config: &["rstest.config.{js,cjs,mjs,ts,cts,mts}"],
    entry: &[TEST_FILES, MOCKS],
    binaries: &["rstest"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Rstest {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let root = value
            .get("root")
            .and_then(ConfigValue::as_str)
            .map_or_else(|| config.base.to_owned(), |root| config.join(root));
        let mut out = Vec::new();
        match value.get("projects") {
            Some(projects) => {
                for project in strings(projects) {
                    for file in project_files(cx, &root, project, PROJECT_CONFIGS) {
                        out.push(PluginInput::ConfigFile(file));
                    }
                }
                for project in objects(projects) {
                    let project_root = project.get("root").and_then(ConfigValue::as_str).map_or_else(
                        || root.clone(),
                        |dir| join_rel(&root, dir.trim_start_matches("<rootDir>/")),
                    );
                    add_project(project, &project_root, config, &mut out);
                }
            }
            None => add_project(value, &root, config, &mut out),
        }
        let provider = get_in(value, &["coverage", "provider"])
            .and_then(ConfigValue::as_str)
            .unwrap_or("istanbul");
        out.push(PluginInput::OptionalDependency(format!("@rstest/coverage-{provider}")));
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        cx.scripts
            .values()
            .flat_map(|script| flag_values(script, SPEC.binaries, &["--testEnvironment"]))
            .filter_map(|name| environment(&name))
            .collect()
    }
}

/// What one project names, with paths relative to its `root`. Excluded and negated patterns are `!` entries.
fn add_project(project: &ConfigValue, root: &str, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    let include = project.get("include").map_or_else(|| vec![TEST_FILES], strings);
    for pattern in include.into_iter().chain([MOCKS]) {
        out.extend(entry_pattern(root, pattern).map(PluginInput::Entry));
    }
    let exclude = project.get("exclude");
    let excluded = exclude
        .and_then(|exclude| exclude.get("patterns"))
        .or(exclude)
        .map_or_default(strings);
    for pattern in excluded {
        out.extend(entry_glob(root, pattern).map(|glob| PluginInput::Entry(format!("!{glob}"))));
    }
    let env = project.get("testEnvironment");
    let env = env.and_then(ConfigValue::as_str).or_else(|| env?.get("name")?.as_str());
    out.extend(env.and_then(environment));
    for key in ["setupFiles", "globalSetup"] {
        for file in project.get(key).map_or_default(strings) {
            out.extend(module_input(root, file, true));
        }
    }
    if get_in(project, &["browser", "enabled"]).and_then(ConfigValue::as_bool) == Some(true) {
        out.push(PluginInput::Dependency("@rstest/browser".to_owned()));
        out.push(PluginInput::Dependency("playwright".to_owned()));
    }
    if let Some(alias) = get_in(project, &["resolve", "alias"]) {
        out.extend(alias_inputs(alias, config));
    }
}

/// The package of a test environment; `node` is built in.
fn environment(name: &str) -> Option<PluginInput> {
    (name != "node").then(|| package_name(name).map(PluginInput::Dependency))?
}
