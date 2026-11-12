//! Nx projects below a package (ADR 0109): the `nx` plugin reads the `project.json` of the package and of its scopes,
//! with paths relative to the workspace root, and a project without a `package.json` of its own is a scope in which the
//! other plugins read their configs, credited to the package.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use skif_core::MemoryFs;
use skif_js_plugins::{
    GlobCache, IncompleteScope, NxProjects, PluginContext, PluginId, PluginOutput, ScopedEntryRegex, resolve_plugins,
    resolve_plugins_in, resolve_plugins_with,
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

fn context<'a>(
    fs: &'a MemoryFs,
    dir: &'a str,
    dependencies: &'a [String],
    empty: &'a IndexMap<String, String>,
    plugins: &'a IndexMap<String, skif_config::PluginConfig>,
) -> PluginContext<'a> {
    static GLOBS: LazyLock<GlobCache> = LazyLock::new(GlobCache::new);
    PluginContext {
        dir: Path::new(dir),
        is_root: dir == DIR,
        package_name: Some("pkg"),
        dependencies,
        inherited_dependencies: &[],
        scripts: empty,
        fs,
        plugins,
        modules: None,
        nx: None,
        globs: &GLOBS,
        sources: None,
    }
}

fn owned(dependencies: &[&str]) -> Vec<String> {
    dependencies.iter().map(|name| (*name).to_owned()).collect()
}

/// Runs the plugins for the root package of `files`, with the Nx projects it finds itself.
fn resolve(files: &[(&str, &str)], dependencies: &[&str]) -> Vec<(PluginId, PluginOutput)> {
    let fs = memory(files);
    let dependencies = owned(dependencies);
    let (scripts, plugins) = (IndexMap::new(), IndexMap::new());
    resolve_plugins(&context(&fs, DIR, &dependencies, &scripts, &plugins))
}

/// Runs the plugins for the package in `dir` (below [`DIR`]), which lies at `path` in the Nx workspace, with the
/// projects `projects` (relative to the package, each with whether the package owns it).
fn resolve_package(
    files: &[(&str, &str)],
    dir: &str,
    path: &str,
    projects: &[(&str, bool)],
) -> Vec<(PluginId, PluginOutput)> {
    let fs = memory(files);
    let dependencies = owned(&["nx"]);
    let (scripts, plugins) = (IndexMap::new(), IndexMap::new());
    let ctx = context(&fs, dir, &dependencies, &scripts, &plugins);
    let found = NxProjects::from_dirs(&ctx, path, || {
        projects
            .iter()
            .map(|(dir, owned)| ((*dir).to_owned(), *owned))
            .collect()
    });
    resolve_plugins_with(&ctx, &found)
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

fn sorted(mut list: Vec<String>) -> Vec<String> {
    list.sort();
    list
}

fn paths(files: &[&str]) -> Vec<PathBuf> {
    files.iter().map(|file| PathBuf::from(DIR).join(file)).collect()
}

/// A project without a `package.json` is a scope: its tool configs are read relative to it, the packages they name
/// are the root's (the module graph credits what the configs import), and the entries they give are relative to the
/// root.
#[test]
fn the_tool_configs_of_an_integrated_project_are_read_for_the_root() {
    let results = resolve(WEB_FILES, ROOT_DEPENDENCIES);

    let vite = output(&results, "vite");
    assert_eq!(vite.config_files, paths(&["apps/web/vite.config.ts"]));
    assert_eq!(vite.production_entries, ["apps/web/src/main.tsx"]);

    let eslint = output(&results, "eslint");
    assert_eq!(eslint.config_files, paths(&["apps/web/eslint.config.mjs"]));

    let jest = output(&results, "jest");
    assert_eq!(jest.config_files, paths(&["apps/web/jest.config.ts"]));
    assert_eq!(names(jest), ["ts-jest"]);
    assert_eq!(
        sorted(jest.entries.clone()),
        [
            "**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "**/__mocks__/**/*.{js,ts,jsx,tsx}",
            "**/__tests__/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "**/{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "apps/web/**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "apps/web/**/__mocks__/**/*.{js,ts,jsx,tsx}",
            "apps/web/**/__tests__/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "apps/web/**/{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "jest.preset.js",
        ]
    );
    assert!(jest.entry_regexes.is_empty(), "{:?}", jest.entry_regexes);
    assert_eq!(jest.scoped_entry_regexes, [ScopedEntryRegex::new("apps/web", "^/e2e/")]);
}

/// Without the `nx` plugin a `project.json` makes no scope, and the root reads no config below it.
#[test]
fn resolving_with_listed_projects_matches_finding_them_first() {
    let fs = memory(WEB_FILES);
    let (scripts, plugins) = (IndexMap::new(), IndexMap::new());
    let listed = || vec![(String::new(), true), ("apps/web".to_owned(), true)];
    for dependencies in [owned(ROOT_DEPENDENCIES), owned(&["vite", "jest"])] {
        let ctx = context(&fs, DIR, &dependencies, &scripts, &plugins);
        let found = NxProjects::from_dirs(&ctx, "", listed);
        let mut asked = false;
        let resolved = resolve_plugins_in(&ctx, "", || {
            asked = true;
            listed()
        });
        assert_eq!(resolved, resolve_plugins_with(&ctx, &found), "{dependencies:?}");
        assert_eq!(asked, dependencies.iter().any(|name| name == "nx"), "{dependencies:?}");
        assert!(!resolved.is_empty(), "{dependencies:?}");
    }
}

#[test]
fn projects_are_scopes_only_with_nx() {
    let dependencies: Vec<&str> = ROOT_DEPENDENCIES.iter().copied().filter(|name| *name != "nx").collect();
    let results = resolve(WEB_FILES, &dependencies);
    assert!(!reads(output(&results, "vite"), "apps/web/vite.config.ts"));
    assert!(!results.iter().any(|(id, _)| *id == "nx"));
}

/// A project with a `package.json` of its own, or below one, is a package's project, not the root's scope: the root
/// reads neither its `project.json` nor its tool configs, while `getJestProjects()` still counts it among the projects.
#[test]
fn a_project_with_its_own_manifest_is_no_scope() {
    let files = [
        ("nx.json", "{}"),
        ("project.json", "{}"),
        ("apps/web/project.json", "{}"),
        ("libs/util/project.json", "{}"),
        ("libs/util/package.json", r#"{ "name": "util" }"#),
        ("libs/util/vite.config.ts", "export default {};\n"),
        ("packages/a/package.json", r#"{ "name": "a" }"#),
        ("packages/a/e2e/project.json", "{}"),
        ("packages/a/e2e/vite.config.ts", "export default {};\n"),
    ];
    let fs = memory(&files);
    let dependencies = owned(ROOT_DEPENDENCIES);
    let (scripts, plugins) = (IndexMap::new(), IndexMap::new());
    let projects = NxProjects::find(&context(&fs, DIR, &dependencies, &scripts, &plugins));
    assert_eq!(projects.path, "");
    assert_eq!(projects.all, ["", "apps/web", "libs/util", "packages/a/e2e"]);
    assert_eq!(projects.dirs, ["", "apps/web"]);
    assert_eq!(projects.scopes, ["apps/web"]);
    let results = resolve(&files, ROOT_DEPENDENCIES);
    assert_eq!(output(&results, "vite").config_files, Vec::<PathBuf>::new());
    assert_eq!(
        output(&results, "nx").config_files,
        paths(&["apps/web/project.json", "nx.json", "project.json"])
    );
}

/// The `nx` plugin reads a `project.json` of the root's anywhere below it, not only under `apps/` and `libs/`, and
/// the commands of its targets run in the workspace root.
#[test]
fn every_project_json_of_the_root_is_read() {
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
            "node_modules/pkg/project.json",
            r#"{ "targets": { "x": { "command": "node node_modules/pkg/run.mjs" } } }"#,
        ),
    ];
    let results = resolve(&files, &["nx"]);
    let nx = output(&results, "nx");
    assert_eq!(nx.entries, ["packages/b/scripts/build.mjs", "tools/c/run.mjs"]);
    assert_eq!(
        nx.config_files,
        paths(&["nx.json", "packages/b/project.json", "tools/c/project.json"])
    );
}

/// A member's own `project.json` gives its paths relative to the workspace root: `main` and `jestConfig` name files
/// of the member, a command without `cwd` runs in the workspace root and one with `cwd: "packages/b"` in the member,
/// where `{workspaceRoot}` is the way back to the root.
#[test]
fn a_member_reads_its_project_json_relative_to_the_workspace_root() {
    let project = r#"{ "targets": {
      "build": { "executor": "@nx/js:tsc", "options": { "main": "packages/b/src/main.ts", "tsConfig": "{projectRoot}/tsconfig.lib.json" } },
      "test": { "executor": "@nx/jest:jest", "options": { "jestConfig": "{workspaceRoot}/packages/b/jest.config.ts" } },
      "gen": { "command": "node packages/b/scripts/gen.mjs" },
      "pack": { "executor": "nx:run-commands", "options": { "command": "node scripts/pack.mjs && node {workspaceRoot}/tools/r.mjs", "cwd": "packages/b" } },
      "other": { "executor": "nx:run-commands", "options": { "command": "node x.mjs", "cwd": "packages/c" } }
    } }"#;
    let files = [
        ("nx.json", "{}"),
        ("package.json", r#"{ "name": "root", "workspaces": ["packages/*"] }"#),
        ("packages/b/package.json", r#"{ "name": "b" }"#),
        ("packages/b/project.json", project),
        ("packages/b/src/main.ts", ""),
        ("packages/b/tsconfig.lib.json", "{}"),
        ("packages/b/jest.config.ts", "export default {};\n"),
        (
            "packages/b/e2e/project.json",
            r#"{ "targets": { "e2e": { "options": { "main": "packages/b/e2e/run.ts" } } } }"#,
        ),
    ];
    let results = resolve_package(&files, "/ws/packages/b", "packages/b", &[("", true), ("e2e", true)]);
    let nx = output(&results, "nx");
    assert_eq!(nx.production_entries, ["e2e/run.ts", "src/main.ts"]);
    assert_eq!(
        sorted(nx.entries.clone()),
        [
            "../../tools/r.mjs",
            "../c/x.mjs",
            "e2e/run.ts",
            "scripts/gen.mjs",
            "scripts/pack.mjs",
            "src/main.ts",
        ]
    );
    assert_eq!(
        nx.config_files,
        paths(&[
            "packages/b/e2e/project.json",
            "packages/b/project.json",
            "packages/b/tsconfig.lib.json",
            "packages/b/jest.config.ts",
        ])
    );
}

/// The root of a package-based workspace reads its own `project.json` and its scopes', not a member's: the member
/// credits its own executors.
#[test]
fn the_root_does_not_read_a_members_project_json() {
    let files = [
        ("nx.json", "{}"),
        ("package.json", r#"{ "name": "root", "workspaces": ["packages/*"] }"#),
        (
            "project.json",
            r#"{ "targets": { "lint": { "executor": "@nx/eslint:lint" } } }"#,
        ),
        ("packages/b/package.json", r#"{ "name": "b" }"#),
        (
            "packages/b/project.json",
            r#"{ "targets": { "build": { "executor": "@nx/vite:build", "options": { "main": "packages/b/src/main.ts" } } } }"#,
        ),
    ];
    let root = resolve_package(&files, DIR, "", &[("", true), ("packages/b", false)]);
    let nx = output(&root, "nx");
    assert_eq!(nx.config_files, paths(&["nx.json", "project.json"]));
    assert_eq!(names(nx), ["@nx/eslint"]);
    assert!(nx.production_entries.is_empty(), "{:?}", nx.production_entries);

    let member = resolve_package(&files, "/ws/packages/b", "packages/b", &[("", true)]);
    let nx = output(&member, "nx");
    assert_eq!(nx.config_files, paths(&["packages/b/project.json"]));
    assert_eq!(names(nx), ["@nx/vite"]);
    assert_eq!(nx.production_entries, ["src/main.ts"]);
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

/// A plugin with neither a config file nor default globs in a scope is not run there, its config-less conventions
/// included (Vite's `index.html`), and a file enabler is matched against each scope's files.
#[test]
fn a_scope_runs_only_the_plugins_its_files_call_for() {
    let files = [
        ("nx.json", "{}"),
        ("apps/web/project.json", "{}"),
        ("apps/web/vite.config.ts", "export default {};\n"),
        ("apps/api/project.json", "{}"),
        ("apps/api/vercel.json", "{}"),
        (
            "apps/api/index.html",
            r#"<html><body><script type="module" src="/src/main.ts"></script></body></html>"#,
        ),
    ];
    let results = resolve(&files, &["nx", "vite"]);
    let vite = output(&results, "vite");
    assert_eq!(vite.config_files, paths(&["apps/web/vite.config.ts"]));
    assert!(vite.production_entries.is_empty(), "{:?}", vite.production_entries);
    let vercel = output(&results, "vercel");
    assert_eq!(vercel.config_files, paths(&["apps/api/vercel.json"]));
    assert_eq!(vercel.entries, ["apps/api/vercel.{js,mjs,cjs,ts,mts}"]);
}

/// A scope whose targets run a tool's Nx executor (`@nx/vite:build`) applies that tool's config-less conventions
/// without a config file, as the executor runs the tool with its defaults: Vite's `index.html` names `src/main.tsx`.
/// A project whose executors are another tool's still does not.
#[test]
fn a_scope_whose_targets_run_a_tools_executor_applies_its_conventions() {
    let html = |main: &str| format!(r#"<html><body><script type="module" src="/src/{main}"></script></body></html>"#);
    let (web, api) = (html("main.tsx"), html("main.ts"));
    let files = [
        ("nx.json", "{}"),
        (
            "apps/web/project.json",
            r#"{ "targets": { "build": { "executor": "@nx/vite:build", "options": { "outputPath": "dist/web" } } } }"#,
        ),
        ("apps/web/index.html", web.as_str()),
        (
            "apps/api/project.json",
            r#"{ "targets": { "build": { "executor": "@nx/js:tsc" } } }"#,
        ),
        ("apps/api/index.html", api.as_str()),
    ];
    let results = resolve(&files, &["nx", "vite", "@nx/vite"]);
    let vite = output(&results, "vite");
    assert!(vite.config_files.is_empty(), "{:?}", vite.config_files);
    assert_eq!(vite.production_entries, ["apps/web/src/main.tsx"]);
}
