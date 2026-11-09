//! Rstest: test files and mocks (`include`, under `root`), the test environment, setup files, browser mode, aliases
//! and the coverage provider of `rstest.config.*`, and the configs or directories its `projects` name.
//!
//! A project with an `exclude` list (`["src/legacy/**"]` or `{ patterns: [...] }`) or a negated `include` pattern has
//! its test files and mocks listed, as knip lists them, so the exclusions apply to this project's files only. The
//! default test and mock globs are entries without a config and next to a config that excludes nothing, as every
//! plugin keeps its conventional entries; a config that excludes files replaces them, or the defaults would bring the
//! excluded files back. The modules a test mocks with `rs.mock("x")` are read by the parser with the other test
//! runners' mock calls.

use globset::{GlobBuilder, GlobMatcher};
use skif_config::PluginConfig;
use skif_js_syntax::ConfigValue;

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::flag_values;
use crate::support::{
    alias_inputs, entry_glob, get_in, join_rel, module_input, objects, package_name, project_files, strings,
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
        let mut excludes = false;
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
                    excludes |= add_project(project, &project_root, config, cx, &mut out);
                }
            }
            None => excludes = add_project(value, &root, config, cx, &mut out),
        }
        if !excludes {
            out.extend(default_entries(cx, config.base));
        }
        let provider = get_in(value, &["coverage", "provider"])
            .and_then(ConfigValue::as_str)
            .unwrap_or("istanbul");
        out.push(PluginInput::OptionalDependency(format!("@rstest/coverage-{provider}")));
        out
    }

    fn resolve_without_config(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        default_entries(cx, "")
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        cx.scripts
            .values()
            .flat_map(|script| flag_values(script, SPEC.binaries, &["--testEnvironment"]))
            .filter_map(|name| environment(&name))
            .collect()
    }
}

/// What one project names, with paths relative to its `root`. Returns whether the project excludes files.
fn add_project(
    project: &ConfigValue,
    root: &str,
    config: &ConfigAst<'_>,
    cx: &PluginContext<'_>,
    out: &mut Vec<PluginInput>,
) -> bool {
    let include = project.get("include").map_or_else(|| vec![TEST_FILES], strings);
    let exclude = project.get("exclude");
    let mut excluded: Vec<String> = exclude
        .and_then(|exclude| exclude.get("patterns"))
        .or(exclude)
        .map(strings)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pattern| entry_glob(root, pattern))
        .collect();
    let mut included = Vec::new();
    for pattern in include.into_iter().chain([MOCKS]) {
        match pattern.trim().strip_prefix('!') {
            Some(negated) => excluded.extend(entry_glob(root, negated)),
            None => included.extend(entry_glob(root, pattern)),
        }
    }
    let excludes = !excluded.is_empty();
    if excludes {
        let excluded: Vec<GlobMatcher> = excluded.iter().filter_map(|pattern| matcher(pattern)).collect();
        out.extend(
            find_files(cx.fs, cx.dir, &included)
                .into_iter()
                .filter(|file| !excluded.iter().any(|glob| glob.is_match(file)))
                .map(PluginInput::Entry),
        );
    } else {
        out.extend(included.into_iter().map(PluginInput::Entry));
    }
    let env = project.get("testEnvironment");
    let env = env.and_then(ConfigValue::as_str).or_else(|| env?.get("name")?.as_str());
    out.extend(env.and_then(environment));
    for key in ["setupFiles", "globalSetup"] {
        for file in project.get(key).map(strings).unwrap_or_default() {
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
    excludes
}

/// The default test and mock globs below `base`, unless `[js.plugins.rstest] entry` replaces them.
fn default_entries(cx: &PluginContext<'_>, base: &str) -> Vec<PluginInput> {
    if let Some(PluginConfig::Options(options)) = cx.plugins.get(SPEC.id)
        && !options.entry.is_empty()
    {
        return Vec::new();
    }
    [TEST_FILES, MOCKS]
        .iter()
        .map(|glob| PluginInput::Entry(join_rel(base, glob)))
        .collect()
}

/// A glob over workspace-relative paths whose `*` stays within one directory; `None` for an invalid glob.
fn matcher(pattern: &str) -> Option<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .ok()
        .map(|glob| glob.compile_matcher())
}

/// The package of a test environment; `node` is built in.
fn environment(name: &str) -> Option<PluginInput> {
    (name != "node").then(|| package_name(name).map(PluginInput::Dependency))?
}
