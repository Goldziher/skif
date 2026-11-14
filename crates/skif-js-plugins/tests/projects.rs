//! `projects` given as strings or globs (the test runners read the config of every project they name), and
//! `tsconfig` `paths` entries with a wildcard that is not the last character.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use skif_core::MemoryFs;
use skif_js_plugins::{GlobCache, PluginContext, PluginOutput, resolve_plugins};

const DIR: &str = "/ws";

fn resolve(files: &[(&str, &str)], dependencies: &[&str], id: &str) -> PluginOutput {
    let mut fs = MemoryFs::new();
    for (path, content) in files {
        fs.insert(format!("{DIR}/{path}"), content);
    }
    let dependencies: Vec<String> = dependencies.iter().map(|name| (*name).to_owned()).collect();
    let scripts = IndexMap::new();
    let plugins = IndexMap::new();
    let ctx = PluginContext {
        dir: Path::new(DIR),
        is_root: true,
        package_name: Some("app"),
        dependencies: &dependencies,
        inherited_dependencies: &[],
        scripts: &scripts,
        fs: &fs,
        plugins: &plugins,
        modules: None,
        nx: None,
        globs: &GlobCache::new(),
        sources: None,
        members: &[],
    };
    resolve_plugins(&ctx)
        .into_iter()
        .find(|(plugin, _)| *plugin == id)
        .map(|(_, output)| output)
        .expect("plugin enabled")
}

fn has_entry(output: &PluginOutput, entry: &str) -> bool {
    output.entries.iter().any(|candidate| candidate == entry)
}

#[test]
fn jest_projects_strings_and_globs_read_the_config_of_each_project() {
    let output = resolve(
        &[
            (
                "jest.config.js",
                "module.exports = { projects: ['<rootDir>', '<rootDir>/packages/*', '<rootDir>/apps/web/'] };\n",
            ),
            (
                "packages/a/jest.config.js",
                "module.exports = { setupFiles: ['<rootDir>/setup.js'], testMatch: ['<rootDir>/spec/**/*.js'] };\n",
            ),
            (
                "packages/b/package.json",
                r#"{"name":"b","jest":{"snapshotSerializers":["./serializer.js"]}}"#,
            ),
            ("apps/web/jest.config.json", r#"{"setupFilesAfterEnv":["./after.js"]}"#),
        ],
        &["jest"],
        "jest",
    );
    for entry in [
        "packages/a/setup.js",
        "packages/a/spec/**/*.js",
        "packages/b/serializer.js",
        "apps/web/after.js",
    ] {
        assert!(has_entry(&output, entry), "{entry}: {:?}", output.entries);
    }
}

/// Nx's `getJestProjects()` and `getJestProjectsAsync()` return the Jest configs of the Nx projects: the
/// `jestConfig` option of a project's target and the `jest.config.*` in a project directory. The root config is read
/// in full, not left incomplete; a `jest.config.*` outside every project is not one of them.
#[test]
fn nx_jest_projects_are_the_jest_configs_of_the_nx_projects() {
    let projects = [
        ("apps/web/project.json", "{}"),
        (
            "apps/web/jest.config.ts",
            "export default { testEnvironment: 'jsdom' };\n",
        ),
        (
            "libs/util/project.json",
            r#"{"targets":{"test":{"executor":"@nx/jest:jest","options":{"jestConfig":"{projectRoot}/jest.unit.ts"}}}}"#,
        ),
        ("libs/util/jest.unit.ts", "export default { preset: 'ts-jest' };\n"),
        (
            "scratch/jest.config.ts",
            "export default { runner: 'jest-runner-eslint' };\n",
        ),
    ];
    for root in [
        "import { getJestProjectsAsync } from '@nx/jest';\nexport default async () => ({ projects: await getJestProjectsAsync() });\n",
        "const { getJestProjects } = require('@nrwl/jest');\nmodule.exports = { projects: getJestProjects() };\n",
    ] {
        let mut files = vec![("jest.config.ts", root)];
        files.extend(projects);
        let output = resolve(&files, &["jest"], "jest");
        assert!(!output.entries_incomplete, "{root}: {:?}", output.incomplete_reasons);
        let names: Vec<&str> = output.referenced_dependencies.iter().map(|d| d.name.as_str()).collect();
        for name in ["jest-environment-jsdom", "ts-jest"] {
            assert!(names.contains(&name), "{root}: missing {name} in {names:?}");
        }
        assert!(!names.contains(&"jest-runner-eslint"), "{root}: {names:?}");
    }
}

/// `getJestProjects()` spread into an array beside a project of the config's own (`[...getJestProjects(),
/// '<rootDir>/tools']`), also awaited, reads the Jest configs of the Nx projects and of the other project, from the
/// projects the `nx` plugin found or, without it, from a walk. An unknown element besides the calls leaves the root
/// incomplete.
#[test]
fn nx_jest_projects_spread_into_an_array() {
    let projects = [
        ("apps/web/project.json", "{}"),
        (
            "apps/web/jest.config.ts",
            "export default { testEnvironment: 'jsdom' };\n",
        ),
        ("tools/jest.config.ts", "export default { preset: 'ts-jest' };\n"),
    ];
    let read: Vec<PathBuf> = ["jest.config.ts", "apps/web/jest.config.ts", "tools/jest.config.ts"]
        .iter()
        .map(|file| Path::new(DIR).join(file))
        .collect();
    for dependencies in [&["jest"][..], &["jest", "nx"][..]] {
        for root in [
            "import { getJestProjects } from '@nx/jest';\nexport default { projects: [...getJestProjects(), '<rootDir>/tools'] };\n",
            "import { getJestProjectsAsync } from '@nx/jest';\nexport default async () => ({ projects: [...(await getJestProjectsAsync()), '<rootDir>/tools'] });\n",
        ] {
            let mut files = vec![("jest.config.ts", root)];
            files.extend(projects);
            let output = resolve(&files, dependencies, "jest");
            assert!(!output.entries_incomplete, "{root}: {:?}", output.incomplete_reasons);
            assert_eq!(output.config_files, read, "{root} with {dependencies:?}");
        }
    }
    let mut files = vec![(
        "jest.config.ts",
        "import { getJestProjects } from '@nx/jest';\nimport { more } from './more';\nexport default { projects: [...getJestProjects(), ...more()] };\n",
    )];
    files.extend(projects);
    assert!(resolve(&files, &["jest"], "jest").entries_incomplete);
}

/// `projects` from another function than Nx's, or Nx's function named without importing it from `@nx/jest`, is not
/// known statically: the root config stays incomplete.
#[test]
fn computed_jest_projects_stay_incomplete() {
    for root in [
        "import { listProjects } from './tools';\nexport default { projects: listProjects() };\n",
        "import { getJestProjects } from './tools';\nexport default { projects: getJestProjects() };\n",
    ] {
        let output = resolve(
            &[("jest.config.ts", root), ("apps/web/project.json", "{}")],
            &["jest"],
            "jest",
        );
        assert!(output.entries_incomplete, "{root}");
    }
}

#[test]
fn vitest_projects_strings_read_the_config_of_each_project() {
    let output = resolve(
        &[
            (
                "vitest.config.ts",
                "export default { test: { projects: ['packages/*', './tools/vitest.config.ts'] } };\n",
            ),
            (
                "packages/a/vite.config.ts",
                "export default { test: { setupFiles: ['./setup.ts'] } };\n",
            ),
            (
                "tools/vitest.config.ts",
                "export default { test: { globalSetup: ['./global.ts'] } };\n",
            ),
        ],
        &["vitest"],
        "vitest",
    );
    for entry in ["packages/a/setup.ts", "tools/global.ts"] {
        assert!(has_entry(&output, entry), "{entry}: {:?}", output.entries);
    }
}

#[test]
fn tsconfig_paths_with_an_inner_wildcard_are_not_followed_as_directories() {
    let output = resolve(
        &[(
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@icons/*":["./icons/*.svg"],"*.svg":["./assets/*.svg"],"@lib/*":["./lib/*"]}}}"#,
        )],
        &["typescript"],
        "typescript",
    );
    let aliases: Vec<(&str, Option<&str>)> = output
        .aliases
        .iter()
        .map(|alias| (alias.find.as_str(), alias.target.as_deref()))
        .collect();
    assert_eq!(aliases, [("@icons", None), ("@lib", Some("lib"))]);
}

#[test]
fn html_templates_skip_placeholders_and_map_the_public_url_to_the_public_directory() {
    let output = resolve(
        &[
            ("webpack.config.js", "module.exports = {};\n"),
            (
                "public/index.html",
                "<script src=\"%PUBLIC_URL%/vendor.js\"></script><script src=\"<%= asset %>\"></script><script src=\"../src/main.js\"></script>",
            ),
        ],
        &["webpack"],
        "webpack",
    );
    assert!(has_entry(&output, "public/vendor.js"), "{:?}", output.entries);
    assert!(has_entry(&output, "src/main.js"), "{:?}", output.entries);
    assert!(
        !output.entries.iter().any(|entry| entry.contains("<%")),
        "{:?}",
        output.entries
    );
}

#[test]
fn browser_extension_overrides_and_sandbox_pages_are_pages() {
    let output = resolve(
        &[
            (
                "manifest.json",
                r#"{"manifest_version":3,"chrome_url_overrides":{"newtab":"newtab.html"},"sandbox":{"pages":["sandbox.html"]},"sidebar_action":{"default_panel":"panel.html"}}"#,
            ),
            ("newtab.html", "<script src=\"newtab.js\"></script>"),
            ("sandbox.html", "<script src=\"sandbox.js\"></script>"),
            ("panel.html", "<script src=\"panel.js\"></script>"),
        ],
        &[],
        "browser-extension",
    );
    for entry in ["newtab.js", "sandbox.js", "panel.js"] {
        assert!(has_entry(&output, entry), "{entry}: {:?}", output.entries);
    }
}
