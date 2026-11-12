//! `[js] isolate-workspaces`: each workspace package uses only the dependencies its own manifest lists, in the default
//! mode as well as in strict mode, which implies it. Strict mode adds production mode and knip's direct-dependency
//! rules (type-only imports use nothing, production code imports from `dependencies`), which isolation alone does not.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

/// The root lists `test-kit`, `shared-types` and `@tsconfig/strictest` for development only; the private `app` lists
/// `own-dep` and, for development, `own-dev`. `app` imports all of them and the workspace member `lib`, which it does
/// not list, and its `tsconfig.json` extends `@tsconfig/strictest`.
const WORKSPACE: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"typescript":"5.0.0","test-kit":"1.0.0","shared-types":"1.0.0","@tsconfig/strictest":"2.0.0"}}"#,
    ),
    (
        "packages/app/package.json",
        r#"{"name":"app","version":"1.0.0","private":true,"main":"src/index.ts","dependencies":{"own-dep":"1.0.0"},"devDependencies":{"own-dev":"1.0.0"}}"#,
    ),
    (
        "packages/app/tsconfig.json",
        r#"{"extends":"@tsconfig/strictest/tsconfig.json"}"#,
    ),
    (
        "packages/app/src/index.ts",
        "import own from 'own-dep';\nimport dev from 'own-dev';\nimport kit from 'test-kit';\nimport type { Shape } from 'shared-types';\nimport lib from 'lib';\nexport const all: Shape = [own, dev, kit, lib];\n",
    ),
    (
        "packages/lib/package.json",
        r#"{"name":"lib","version":"1.0.0","main":"src/index.ts"}"#,
    ),
    ("packages/lib/src/index.ts", "export default 1;\n"),
];

fn lint(toml: &str) -> Vec<Diagnostic> {
    lint_files(WORKSPACE, toml)
}

fn lint_files(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `<path>: <dependency>` of every diagnostic of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display().to_string().replace('\\', "/"),
                d.dependency.clone().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

/// Without isolation the root's development dependencies and the private package's member import satisfy `app`.
/// Isolation outside production mode reports each of them: the import of the root-only development dependency
/// `test-kit`, the type-only import of `shared-types`, the unlisted member `lib` and the `tsconfig` that extends a
/// package only the root lists. `app`'s own development dependency stays fine there. Strict mode is production mode,
/// so no config is judged, and it adds knip's direct-dependency rules: the type-only import uses nothing, and the
/// production import of `own-dev` needs it in `dependencies`.
#[test]
fn isolation_checks_every_package_against_its_own_manifest_in_the_default_mode() {
    let cases: [(&str, &[&str]); 4] = [
        ("", &[]),
        (
            "[js]\nisolate-workspaces = true\n",
            &[
                "packages/app/src/index.ts: lib",
                "packages/app/src/index.ts: shared-types",
                "packages/app/src/index.ts: test-kit",
                "packages/app/tsconfig.json: @tsconfig/strictest",
            ],
        ),
        (
            "[js]\nstrict = true\n",
            &[
                "packages/app/src/index.ts: lib",
                "packages/app/src/index.ts: own-dev",
                "packages/app/src/index.ts: test-kit",
            ],
        ),
        (
            "[js]\nproduction = true\nisolate-workspaces = true\n",
            &[
                "packages/app/src/index.ts: lib",
                "packages/app/src/index.ts: shared-types",
                "packages/app/src/index.ts: test-kit",
            ],
        ),
    ];
    for (toml, expected) in cases {
        assert_eq!(found(&lint(toml), "unlisted-dependencies"), expected, "{toml:?}");
    }
}

/// Isolation outside production mode keeps checking development dependencies: the root's `test-kit`, which only a
/// member imports, is used by no package that lists it. Strict mode, which is production mode, checks none.
#[test]
fn isolation_without_production_mode_reports_an_ancestor_development_dependency_only_members_use() {
    let unused = "package.json: test-kit".to_owned();
    let default = found(&lint(""), "unused-dev-dependencies");
    assert!(!default.contains(&unused), "{default:?}");
    let isolated = found(&lint("[js]\nisolate-workspaces = true\n"), "unused-dev-dependencies");
    assert!(isolated.contains(&unused), "{isolated:?}");
    assert_eq!(
        found(&lint("[js]\nstrict = true\n"), "unused-dev-dependencies"),
        Vec::<String>::new()
    );
}

/// Isolation keeps a package's own development dependencies available to its tests, its tool configs and its
/// `tsconfig.json`: only what an ancestor lists stops counting.
#[test]
fn isolation_lets_tests_and_configs_use_the_packages_own_development_dependencies() {
    let files: &[(&str, &str)] = &[
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"typescript":"5.0.0","root-kit":"1.0.0"}}"#,
        ),
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","private":true,"main":"src/index.ts","devDependencies":{"vitest":"3.0.0","own-plugin":"1.0.0","own-dev":"1.0.0","@tsconfig/node22":"22.0.0"}}"#,
        ),
        (
            "packages/app/tsconfig.json",
            r#"{"extends":"@tsconfig/node22/tsconfig.json"}"#,
        ),
        (
            "packages/app/vitest.config.ts",
            "import plugin from 'own-plugin';\nexport default { plugins: [plugin()] };\n",
        ),
        ("packages/app/src/index.ts", "export const one = 1;\n"),
        (
            "packages/app/src/index.test.ts",
            "import dev from 'own-dev';\nimport kit from 'root-kit';\nimport { one } from './index';\nexport const all = [dev, kit, one];\n",
        ),
    ];
    assert_eq!(
        found(&lint_files(files, ""), "unlisted-dependencies"),
        Vec::<String>::new()
    );
    assert_eq!(
        found(
            &lint_files(files, "[js]\nisolate-workspaces = true\n"),
            "unlisted-dependencies"
        ),
        ["packages/app/src/index.test.ts: root-kit"]
    );
}
