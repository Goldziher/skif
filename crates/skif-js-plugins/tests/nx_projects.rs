//! Nx projects below a package (ADR 0109): the `nx` plugin reads every `project.json`, and a project without a
//! `package.json` of its own is a scope in which the other plugins read their configs, credited to the package.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use skif_core::MemoryFs;
use skif_js_plugins::{
    IncompleteScope, NxProjects, PluginContext, PluginId, PluginOutput, resolve_plugins, resolve_plugins_with,
};

const DIR: &str = "/ws";

/// The integrated workspace's root dependencies.
const ROOT_DEPENDENCIES: &[&str] = &[
    "nx",
    "vite",
    "@vitejs/plugin-react",
    "eslint",
    "eslint-plugin-react",
    "jest",
    "ts-jest",
    "typescript",
];

fn memory(files: &[(&str, &str)]) -> MemoryFs {
    let mut fs = MemoryFs::new();
    for (path, content) in files {
        fs.insert(format!("{DIR}/{path}"), content);
    }
    fs
}

/// Runs the plugins for the root package of `files`; `projects` picks the Nx projects from those found.
fn resolve_root(
    files: &[(&str, &str)],
    dependencies: &[&str],
    projects: impl Fn(NxProjects) -> NxProjects,
) -> Vec<(PluginId, PluginOutput)> {
    let fs = memory(files);
    let dependencies: Vec<String> = dependencies.iter().map(|name| (*name).to_owned()).collect();
    let scripts = IndexMap::new();
    let plugins = IndexMap::new();
    let ctx = PluginContext {
        dir: Path::new(DIR),
        is_root: true,
        package_name: Some("root"),
        dependencies: &dependencies,
        inherited_dependencies: &[],
        scripts: &scripts,
        fs: &fs,
        plugins: &plugins,
        modules: None,
    };
    resolve_plugins_with(&ctx, &projects(NxProjects::find(&ctx)))
}

fn resolve(files: &[(&str, &str)], dependencies: &[&str]) -> Vec<(PluginId, PluginOutput)> {
    resolve_root(files, dependencies, |projects| projects)
}

fn output<'a>(results: &'a [(PluginId, PluginOutput)], id: &str) -> &'a PluginOutput {
    results
        .iter()
        .find(|(plugin, _)| *plugin == id)
        .map_or_else(|| panic!("plugin {id} is not enabled"), |(_, output)| output)
}

fn names(output: &PluginOutput) -> Vec<&str> {
    let mut names: Vec<&str> = output
        .referenced_dependencies
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn reads(output: &PluginOutput, file: &str) -> bool {
    output.config_files.contains(&PathBuf::from(DIR).join(file))
}

const WEB_FILES: &[(&str, &str)] = &[
    ("nx.json", "{}"),
    ("apps/web/project.json", r#"{ "name": "web" }"#),
    (
        "apps/web/vite.config.ts",
        "import react from '@vitejs/plugin-react';\nexport default { root: __dirname, plugins: [react()] };\n",
    ),
    (
        "apps/web/index.html",
        r#"<html><body><script type="module" src="/src/main.tsx"></script></body></html>"#,
    ),
    (
        "apps/web/eslint.config.mjs",
        "import react from 'eslint-plugin-react';\nexport default [{ plugins: { react } }];\n",
    ),
    (
        "apps/web/jest.config.ts",
        "export default { preset: '../../jest.preset.js', transform: { '^.+\\\\.ts$': 'ts-jest' }, testRegex: '^/e2e/' };\n",
    ),
    ("jest.preset.js", "module.exports = {};\n"),
];

/// A project without a `package.json` is a scope: its tool configs are read relative to it, the packages they name
/// are the root's (the module graph credits what the configs import), and the entries they give are relative to the
/// root.
#[test]
fn the_tool_configs_of_an_integrated_project_are_read_for_the_root() {
    let results = resolve(WEB_FILES, ROOT_DEPENDENCIES);

    let vite = output(&results, "vite");
    assert!(reads(vite, "apps/web/vite.config.ts"), "{:?}", vite.config_files);
    assert!(
        vite.production_entries.contains(&"apps/web/src/main.tsx".to_owned()),
        "{:?}",
        vite.production_entries
    );

    let eslint = output(&results, "eslint");
    assert!(reads(eslint, "apps/web/eslint.config.mjs"), "{:?}", eslint.config_files);

    let jest = output(&results, "jest");
    assert!(reads(jest, "apps/web/jest.config.ts"), "{:?}", jest.config_files);
    assert!(names(jest).contains(&"ts-jest"), "{:?}", names(jest));
    assert!(
        jest.entries.contains(&"jest.preset.js".to_owned()),
        "{:?}",
        jest.entries
    );
    assert!(
        jest.entries
            .contains(&"apps/web/**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}".to_owned()),
        "{:?}",
        jest.entries
    );
    assert_eq!(jest.entry_regexes, ["^/apps/web(?:/e2e/)"]);
}

/// Without the `nx` plugin a `project.json` makes no scope, and the root reads no config below it.
#[test]
fn projects_are_scopes_only_with_nx() {
    let dependencies: Vec<&str> = ROOT_DEPENDENCIES.iter().copied().filter(|name| *name != "nx").collect();
    let results = resolve(WEB_FILES, &dependencies);
    assert!(!reads(output(&results, "vite"), "apps/web/vite.config.ts"));
    assert!(!results.iter().any(|(id, _)| *id == "nx"));
}

/// A project with a `package.json` of its own, or below one, is a package's project, not the root's scope; its
/// `project.json` is still read from the root.
#[test]
fn a_project_with_its_own_manifest_is_no_scope() {
    let files = [
        ("nx.json", "{}"),
        ("libs/util/project.json", "{}"),
        ("libs/util/package.json", r#"{ "name": "util" }"#),
        ("libs/util/vite.config.ts", "export default {};\n"),
        ("packages/a/package.json", r#"{ "name": "a" }"#),
        ("packages/a/e2e/project.json", "{}"),
        ("packages/a/e2e/vite.config.ts", "export default {};\n"),
    ];
    let fs = memory(&files);
    let dependencies: Vec<String> = ROOT_DEPENDENCIES.iter().map(|name| (*name).to_owned()).collect();
    let scripts = IndexMap::new();
    let plugins = IndexMap::new();
    let ctx = PluginContext {
        dir: Path::new(DIR),
        is_root: true,
        package_name: Some("root"),
        dependencies: &dependencies,
        inherited_dependencies: &[],
        scripts: &scripts,
        fs: &fs,
        plugins: &plugins,
        modules: None,
    };
    let projects = NxProjects::find(&ctx);
    assert_eq!(projects.dirs, ["libs/util", "packages/a/e2e"]);
    assert!(projects.scopes.is_empty(), "{:?}", projects.scopes);
    let results = resolve(&files, ROOT_DEPENDENCIES);
    let vite = output(&results, "vite");
    assert!(!reads(vite, "libs/util/vite.config.ts"));
    assert!(!reads(vite, "packages/a/e2e/vite.config.ts"));
    assert!(reads(output(&results, "nx"), "libs/util/project.json"));
}

/// The `nx` plugin reads a `project.json` anywhere below the package, not only under `apps/` and `libs/`, and the
/// commands of its targets run in the workspace root; a project the caller leaves out is not read.
#[test]
fn every_project_json_is_read_from_the_package() {
    let files = [
        ("nx.json", "{}"),
        (
            "packages/b/project.json",
            r#"{ "targets": { "build": { "command": "node packages/b/scripts/build.mjs" } } }"#,
        ),
        (
            "tools/c/project.json",
            r#"{ "targets": { "x": { "executor": "nx:run-commands", "options": { "command": "node {projectRoot}/run.mjs" } } } }"#,
        ),
        (
            "out/stale/project.json",
            r#"{ "targets": { "x": { "command": "node out/stale/run.mjs" } } }"#,
        ),
        (
            "node_modules/pkg/project.json",
            r#"{ "targets": { "x": { "command": "node node_modules/pkg/run.mjs" } } }"#,
        ),
    ];
    let results = resolve_root(&files, &["nx"], |projects| {
        projects.filter(|dir| !dir.starts_with("out/"))
    });
    let nx = output(&results, "nx");
    for entry in ["packages/b/scripts/build.mjs", "tools/c/run.mjs"] {
        assert!(
            nx.entries.contains(&entry.to_owned()),
            "missing {entry}: {:?}",
            nx.entries
        );
    }
    for entry in ["out/stale/run.mjs", "node_modules/pkg/run.mjs"] {
        assert!(
            !nx.entries.contains(&entry.to_owned()),
            "unexpected {entry}: {:?}",
            nx.entries
        );
    }
    // The plain entry point finds every project itself.
    let fs = memory(&files);
    let dependencies = vec!["nx".to_owned()];
    let scripts = IndexMap::new();
    let plugins = IndexMap::new();
    let ctx = PluginContext {
        dir: Path::new(DIR),
        is_root: true,
        package_name: Some("root"),
        dependencies: &dependencies,
        inherited_dependencies: &[],
        scripts: &scripts,
        fs: &fs,
        plugins: &plugins,
        modules: None,
    };
    let all = resolve_plugins(&ctx);
    assert!(output(&all, "nx").entries.contains(&"out/stale/run.mjs".to_owned()));
}

/// Entries a config of a scope hides lie in the scope: a config of a plugin that cannot bound them leaves the
/// project's files unjudged, not the whole package's.
#[test]
fn an_incomplete_config_of_a_scope_is_bounded_by_the_scope() {
    let files = [
        ("nx.json", "{}"),
        ("apps/web/project.json", "{}"),
        ("apps/web/eslint.config.mjs", "export default compute();\n"),
    ];
    let results = resolve(&files, ROOT_DEPENDENCIES);
    let eslint = output(&results, "eslint");
    assert!(eslint.entries_incomplete);
    let scopes: Vec<&IncompleteScope> = eslint.incomplete_reasons.iter().map(|reason| &reason.scope).collect();
    assert_eq!(scopes, [&IncompleteScope::Within(vec!["apps/web/**".to_owned()])]);
}
