//! `projects` given as strings or globs (the test runners read the config of every project they name), and
//! `tsconfig` `paths` entries with a wildcard that is not the last character.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use indexmap::IndexMap;
use skif_core::MemoryFs;
use skif_js_plugins::{PluginContext, PluginOutput, resolve_plugins};

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
