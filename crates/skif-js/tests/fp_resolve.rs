//! False positives of the dependency and resolution rules: binaries from ancestor projects, lockfile facts, on-demand
//! tools, ambient `@types`, declaration files, generated targets and root-relative paths.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs, Report};
use skif_js::{InputSource, JsAnalysis, JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run, run_unsorted};
use skif_workspace::Workspace;

fn lint_at(root: &str, files: &[(&str, &str)]) -> Vec<Diagnostic> {
    lint_configured(root, files, "")
}

/// The diagnostics of the workspace at `root` with the `skif.toml` text `toml`.
fn lint_configured(root: &str, files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files
        .iter()
        .fold(MemoryFs::new(), |memory, (path, text)| memory.with_file(path, text));
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(root), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// The sorted report of the workspace at `root`, with the files and extra entries a multi-root host reads.
fn report_at(root: &str, files: &[(&str, &str)]) -> Report {
    report_with(root, files, &RunOptions::default())
}

/// Like [`report_at`], run with `options`: the extra entries and build sources a multi-root host hands a root.
fn report_with(root: &str, files: &[(&str, &str)], options: &RunOptions<'_>) -> Report {
    let memory = files
        .iter()
        .fold(MemoryFs::new(), |memory, (path, text)| memory.with_file(path, text));
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(root), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run(&registry, &workspace, &config, options).unwrap()
}

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    lint_with(files, "")
}

/// The diagnostics of the workspace at `/ws` holding `files`, relative to it, with the `skif.toml` text `toml`.
fn lint_with(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let rooted: Vec<(String, &str)> = files
        .iter()
        .map(|(path, text)| (format!("/ws/{path}"), *text))
        .collect();
    let borrowed: Vec<(&str, &str)> = rooted.iter().map(|(path, text)| (path.as_str(), *text)).collect();
    lint_configured("/ws", &borrowed, toml)
}

/// The names the diagnostics of `rule` are about.
fn names(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            d.dependency.clone().unwrap_or_else(|| {
                let start = d.message.find('`').map_or(0, |i| i + 1);
                let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
                d.message[start..end].to_owned()
            })
        })
        .collect();
    found.sort();
    found
}

fn paths(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| d.location.path.to_string_lossy().replace('\\', "/"))
        .collect();
    found.sort();
    found
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

const PARENT: &str = r#"{"name":"repo","private":true,"devDependencies":{"oxlint":"1.0.0","oxfmt":"0.1.0"}}"#;
const CHILD: &str = r#"{"name":"child","private":true,"scripts":{"check":"oxlint . && oxfmt --check ."}}"#;

#[test]
fn binaries_of_ancestor_projects_are_on_the_path() {
    let diagnostics = lint_at(
        "/ws/child",
        &[("/ws/package.json", PARENT), ("/ws/child/package.json", CHILD)],
    );
    assert_eq!(
        names(&diagnostics, "unlisted-binaries"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn only_binaries_an_ancestor_lists_are_provided() {
    let parent = r#"{"name":"repo","private":true,"devDependencies":{"oxlint":"1.0.0"}}"#;
    let diagnostics = lint_at(
        "/ws/child",
        &[("/ws/package.json", parent), ("/ws/child/package.json", CHILD)],
    );
    assert_eq!(names(&diagnostics, "unlisted-binaries"), strings(&["oxfmt"]));
}

#[test]
fn ancestors_above_the_repository_root_are_not_searched() {
    let diagnostics = lint_at(
        "/ws/child",
        &[
            ("/package.json", PARENT),
            ("/ws/.git/HEAD", ""),
            ("/ws/child/package.json", CHILD),
        ],
    );
    assert_eq!(names(&diagnostics, "unlisted-binaries"), strings(&["oxfmt", "oxlint"]));
}

#[test]
fn ancestor_manifests_make_imports_listed() {
    let child = r#"{"name":"child","private":true,"main":"index.js"}"#;
    let diagnostics = lint_at(
        "/ws/child",
        &[
            ("/ws/package.json", PARENT),
            ("/ws/child/package.json", child),
            ("/ws/child/index.js", "import 'oxlint';\nimport 'other';\n"),
        ],
    );
    assert_eq!(names(&diagnostics, "unlisted-dependencies"), strings(&["other"]));
}

#[test]
fn uses_through_ancestor_manifests_are_reported_with_the_innermost_manifest() {
    // `left-pad` is listed by both ancestors, so Node.js resolves it from the inner one; `oxfmt` only the outer lists,
    // and the script runs it. `own` is the child's own dependency and `idle` nobody uses.
    let middle = r#"{"name":"middle","private":true,"dependencies":{"left-pad":"1.0.0"}}"#;
    let outer = r#"{"name":"repo","private":true,"dependencies":{"left-pad":"1.0.0","idle":"1.0.0"},
        "devDependencies":{"oxfmt":"0.1.0"}}"#;
    let child = r#"{"name":"child","private":true,"main":"index.js","dependencies":{"own":"1.0.0"},
        "scripts":{"format":"oxfmt ."}}"#;
    let report = report_at(
        "/ws/tools/child",
        &[
            ("/ws/.git/HEAD", ""),
            ("/ws/package.json", outer),
            ("/ws/tools/package.json", middle),
            ("/ws/tools/child/package.json", child),
            ("/ws/tools/child/index.js", "import 'left-pad';\nimport 'own';\n"),
        ],
    );
    assert_eq!(
        report.ancestor_dependency_uses,
        [
            (PathBuf::from("/ws"), "oxfmt".to_owned()),
            (PathBuf::from("/ws/tools"), "left-pad".to_owned())
        ]
    );
}

/// A binary that a script or a workflow step runs after `cd` into a project of its own below the workspace (a directory
/// with a `package.json` that is no workspace member, such as a nested root of a recursive run) is judged against that
/// project's manifest: a provider it lists is used through it, for the multi-root host to credit, and nothing is
/// unlisted at the command; one it does not provide is unlisted at the command, naming the project.
#[test]
fn binaries_run_after_cd_into_a_project_of_its_own_are_judged_against_its_manifest() {
    let root = r#"{"name":"repo","private":true,"scripts":{"css":"cd build/rspack && rspack build"}}"#;
    let project = r#"{"name":"rspack-build","private":true,"devDependencies":{"@rspack/cli":"1.0.0"}}"#;
    let workflow = "name: css\non: [push]\njobs:\n  scan:\n    runs-on: ubuntu-latest\n    steps:\n      \
                    - run: cd build/rspack && pnpm exec rspack build\n      \
                    - run: cd build/rspack && pnpm exec webpack serve\n";
    let report = report_at(
        "/ws",
        &[
            ("/ws/.git/HEAD", ""),
            ("/ws/package.json", root),
            ("/ws/build/rspack/package.json", project),
            ("/ws/.github/workflows/css.yml", workflow),
        ],
    );
    let unlisted: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "unlisted-binaries")
        .map(|d| d.message.as_str())
        .collect();
    let expected = "`.github/workflows/css.yml` runs `webpack` in `build/rspack`, which no dependency of \
                    `rspack-build` provides";
    assert_eq!(unlisted, [expected], "{:#?}", report.diagnostics);
    assert_eq!(
        report.ancestor_dependency_uses,
        [(PathBuf::from("/ws/build/rspack"), "@rspack/cli".to_owned())]
    );
}

/// Two standalone projects side by side: `b` requires `../a/lib.js`, which requires `./helper.js` and `lodash`, a
/// dependency of `a`.
const SIBLINGS: [(&str, &str); 6] = [
    ("/ws/.git/HEAD", ""),
    (
        "/ws/a/package.json",
        r#"{"name":"a","private":true,"main":"index.js","dependencies":{"lodash":"4.17.21"}}"#,
    ),
    ("/ws/a/index.js", "module.exports = 1;\n"),
    (
        "/ws/a/lib.js",
        "require('./helper.js');\nmodule.exports = require('lodash');\n",
    ),
    ("/ws/a/helper.js", "module.exports = 2;\n"),
    ("/ws/b/package.json", r#"{"name":"b","private":true,"main":"index.js"}"#),
];

#[test]
fn files_outside_the_root_are_followed_and_listed_absolute_without_findings_of_their_own() {
    let mut files = SIBLINGS.to_vec();
    files.push(("/ws/b/index.js", "module.exports = require('../a/lib.js');\n"));
    let report = report_at("/ws/b", &files);
    assert_eq!(
        report.other_project_files,
        [PathBuf::from("/ws/a/helper.js"), PathBuf::from("/ws/a/lib.js")]
    );
    // `lib.js` belongs to `a`, which judges it against its own manifest: `b` lists no `lodash` and is not told to.
    let findings: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|d| d.rule.as_str())
        .filter(|rule| ["unlisted-dependencies", "unresolved-imports", "unused-files"].contains(rule))
        .collect();
    assert_eq!(findings, Vec::<&str>::new(), "{:?}", report.diagnostics);
    // Node.js resolves `lodash` for `lib.js` from `a`, so `a`'s manifest is where the use counts.
    assert_eq!(
        report.ancestor_dependency_uses,
        [(PathBuf::from("/ws/a"), "lodash".to_owned())]
    );
}

#[test]
fn manifests_above_the_repository_root_do_not_make_imports_listed() {
    let child = r#"{"name":"child","private":true,"main":"index.js"}"#;
    let diagnostics = lint_at(
        "/ws/child",
        &[
            ("/package.json", PARENT),
            ("/ws/.git/HEAD", ""),
            ("/ws/child/package.json", child),
            ("/ws/child/index.js", "import 'oxlint';\n"),
        ],
    );
    assert_eq!(names(&diagnostics, "unlisted-dependencies"), strings(&["oxlint"]));
}

const TESTING_APP: &str = r#"{"name":"app","private":true,"dependencies":{"react":"18.0.0"},"devDependencies":{"@testing-library/dom":"10.0.0","@testing-library/react":"16.0.0","vitest":"3.0.0"}}"#;
const TESTING_TEST: &str = "import { render } from '@testing-library/react';\nimport { it } from 'vitest';\nit('x', () => { render(null as never); });\n";
const PNPM_LOCK: &str = "lockfileVersion: '9.0'\n\npackages:\n\n  '@testing-library/react@16.0.0(@testing-library/dom@10.0.0)(react@18.0.0)':\n    resolution: {integrity: sha512-x}\n    peerDependencies:\n      '@testing-library/dom': ^10.0.0\n      react: ^18.0.0\n\nsnapshots:\n";

#[test]
fn peers_come_from_the_lockfile_when_nothing_is_installed() {
    let without = lint(&[("package.json", TESTING_APP), ("src/a.test.tsx", TESTING_TEST)]);
    assert!(
        names(&without, "unused-dev-dependencies").contains(&"@testing-library/dom".to_owned()),
        "{without:#?}"
    );
    let with = lint(&[
        ("package.json", TESTING_APP),
        ("src/a.test.tsx", TESTING_TEST),
        ("pnpm-lock.yaml", PNPM_LOCK),
    ]);
    assert_eq!(
        names(&with, "unused-dev-dependencies"),
        Vec::<String>::new(),
        "{with:#?}"
    );
    assert_eq!(names(&with, "unused-dependencies"), Vec::<String>::new(), "{with:#?}");
}

#[test]
fn peers_come_from_the_npm_lockfile_too() {
    let lock = r#"{"lockfileVersion":3,"packages":{"":{},"node_modules/@testing-library/react":{"peerDependencies":{"@testing-library/dom":"^10.0.0","react":"^18"}}}}"#;
    let diagnostics = lint(&[
        ("package.json", TESTING_APP),
        ("src/a.test.tsx", TESTING_TEST),
        ("package-lock.json", lock),
    ]);
    assert_eq!(
        names(&diagnostics, "unused-dev-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn peers_come_from_bun_and_yarn_lockfiles_too() {
    let bun = r#"{"lockfileVersion":1,"packages":{"@testing-library/react":["@testing-library/react@16.0.0","",{"peerDependencies":{"@testing-library/dom":"^10.0.0","react":"^18",},},"sha512-x"],}}"#;
    let yarn = "__metadata:\n  version: 8\n\n\"@testing-library/react@npm:^16.0.0\":\n  version: 16.0.0\n  peerDependencies:\n    \"@testing-library/dom\": ^10.0.0\n    react: ^18.0.0\n";
    for (lockfile, text) in [("bun.lock", bun), ("yarn.lock", yarn)] {
        let diagnostics = lint(&[
            ("package.json", TESTING_APP),
            ("src/a.test.tsx", TESTING_TEST),
            (lockfile, text),
        ]);
        assert_eq!(
            names(&diagnostics, "unused-dev-dependencies"),
            Vec::<String>::new(),
            "{lockfile}: {diagnostics:#?}"
        );
    }
}

#[test]
fn a_classic_yarn_lock_does_not_hide_the_binaries_of_used_dev_dependencies() {
    let manifest = r#"{"name":"app","private":true,"packageManager":"yarn@1.22.19","main":"src/index.js","scripts":{"t":"vitest run","p":"prettier -w ."},"devDependencies":{"vitest":"1.0.0","prettier":"3.0.0"}}"#;
    let classic = "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n# yarn lockfile v1\n\n\nprettier@3.0.0:\n  version \"3.0.0\"\n  resolved \"https://registry.yarnpkg.com/prettier/-/prettier-3.0.0.tgz\"\n  integrity sha512-x\n\nvitest@1.0.0:\n  version \"1.0.0\"\n  resolved \"https://registry.yarnpkg.com/vitest/-/vitest-1.0.0.tgz\"\n  integrity sha512-y\n";
    let diagnostics = lint(&[
        ("package.json", manifest),
        ("src/index.js", "export const a = 1;\n"),
        ("yarn.lock", classic),
    ]);
    assert_eq!(
        names(&diagnostics, "unused-dev-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unlisted-binaries"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn peers_of_a_workspace_package_count_without_an_install() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "packages/testing/package.json",
            r#"{"name":"@fx/testing","version":"1.0.0","main":"index.ts","peerDependencies":{"@testing-library/dom":"^10.0.0"}}"#,
        ),
        ("packages/testing/index.ts", "export const render = () => null;\n"),
        (
            "packages/app/package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"@fx/testing":"workspace:*","@testing-library/dom":"10.0.0"}}"#,
        ),
        (
            "packages/app/src/index.ts",
            "import { render } from '@fx/testing';\nrender();\n",
        ),
    ]);
    assert_eq!(
        names(&diagnostics, "unused-dev-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

const TOOLS: &str = r#"{"name":"app","private":true,"packageManager":"pnpm@10.0.0","scripts":{"e2e":"npx playwright test","start":"webpack serve --mode development","ui":"vitest --ui","cov":"vitest run --coverage","deploy":"node node_modules/@google/clasp deploy","filtered":"pnpm --filter web build","fetch":"npx cowsay hi","each":"pnpm -r --filter web exec vite"},"dependencies":{"@google/clasp":"1.0.0","playwright":"1.0.0"},"devDependencies":{"@vitest/coverage-v8":"3.0.0","@vitest/ui":"3.0.0","pnpm":"10.0.0","vitest":"3.0.0","webpack":"5.0.0","webpack-cli":"5.0.0","webpack-dev-server":"5.0.0"}}"#;

#[test]
fn on_demand_tools_and_flags_credit_their_packages() {
    let diagnostics = lint(&[("package.json", TOOLS)]);
    assert_eq!(
        names(&diagnostics, "unused-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-dev-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unlisted-binaries"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn unlisted_tools_are_still_unused_when_nothing_runs_them() {
    let manifest = r#"{"name":"app","private":true,"scripts":{"build":"tsc"},"devDependencies":{"@vitest/ui":"3.0.0","typescript":"5.0.0"}}"#;
    let diagnostics = lint(&[("package.json", manifest)]);
    assert_eq!(names(&diagnostics, "unused-dev-dependencies"), strings(&["@vitest/ui"]));
}

fn types_manifest() -> &'static str {
    r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"@types/chrome":"0.0.1","typescript":"5.0.0"}}"#
}

#[test]
fn every_listed_types_package_is_ambient_without_a_types_option() {
    for tsconfig in [
        r#"{"compilerOptions":{"strict":true}}"#,
        r#"{"compilerOptions":{"typeRoots":["node_modules/@types"]}}"#,
        r#"{"extends":"./base.json"}"#,
        // TypeScript 6 spells "every package of the type roots" as `*`.
        r#"{"compilerOptions":{"types":["*"]}}"#,
    ] {
        let diagnostics = lint(&[
            ("package.json", types_manifest()),
            ("tsconfig.json", tsconfig),
            ("base.json", r#"{"compilerOptions":{"strict":true}}"#),
            ("src/index.ts", "export const a = 1;\n"),
        ]);
        assert_eq!(
            names(&diagnostics, "unused-dev-dependencies"),
            Vec::<String>::new(),
            "{tsconfig}: {diagnostics:#?}"
        );
    }
}

#[test]
fn types_packages_are_not_ambient_when_the_tsconfig_narrows_them() {
    for tsconfig in [
        Some(r#"{"compilerOptions":{"types":["node"]}}"#),
        Some(r#"{"compilerOptions":{"typeRoots":["./typings"]}}"#),
        Some(r#"{"extends":"./base.json"}"#),
        None,
    ] {
        let mut files = vec![
            ("package.json", types_manifest()),
            ("base.json", r#"{"compilerOptions":{"types":["node"]}}"#),
            ("src/index.ts", "export const a = 1;\n"),
        ];
        if let Some(tsconfig) = tsconfig {
            files.push(("tsconfig.json", tsconfig));
        }
        let diagnostics = lint(&files);
        assert_eq!(
            names(&diagnostics, "unused-dev-dependencies"),
            strings(&["@types/chrome"]),
            "{tsconfig:?}: {diagnostics:#?}"
        );
    }
}

/// A manifest listing `@types/chrome` and `typescript` at `range`.
fn types_manifest_with(range: &str) -> String {
    format!(
        r#"{{"name":"app","private":true,"main":"src/index.ts","devDependencies":{{"@types/chrome":"0.0.1","typescript":"{range}"}}}}"#
    )
}

/// The manifest of an installed `typescript` at `version`.
fn installed_typescript(version: &str) -> String {
    format!(r#"{{"name":"typescript","version":"{version}","bin":{{"tsc":"./bin/tsc","tsserver":"./bin/tsserver"}}}}"#)
}

/// The `unused-dev-dependencies` of a package listing `@types/chrome` and `typescript` at `range`, with `typescript`
/// at `installed` in its `node_modules`, under `tsconfig`.
fn unused_types(range: &str, installed: Option<&str>, tsconfig: &str) -> Vec<String> {
    let manifest = types_manifest_with(range);
    let installed = installed.map(installed_typescript);
    let mut files = vec![
        ("package.json", manifest.as_str()),
        ("tsconfig.json", tsconfig),
        ("base.json", r#"{"compilerOptions":{"types":["*"]}}"#),
        ("src/index.ts", "export const a = 1;\n"),
    ];
    if let Some(installed) = &installed {
        files.push(("node_modules/typescript/package.json", installed.as_str()));
    }
    names(&lint(&files), "unused-dev-dependencies")
}

const PLAIN_TSCONFIG: &str = r#"{"compilerOptions":{"strict":true}}"#;

#[test]
fn types_packages_stay_ambient_without_a_types_option_before_typescript_6() {
    // The installed version wins over the declared range; an unknown version keeps every package ambient.
    for (range, installed) in [
        ("^5.4.0", None),
        ("5.9.3", Some("5.9.3")),
        ("^6.0.0", Some("5.9.3")),
        ("latest", None),
    ] {
        assert_eq!(
            unused_types(range, installed, PLAIN_TSCONFIG),
            Vec::<String>::new(),
            "{range} {installed:?}"
        );
    }
}

#[test]
fn types_packages_are_not_ambient_without_a_types_option_on_an_installed_typescript_6() {
    for (range, installed) in [("^6.0.0", "6.0.2"), ("^5.4.0", "6.0.2"), ("latest", "6.0.2")] {
        assert_eq!(
            unused_types(range, Some(installed), PLAIN_TSCONFIG),
            strings(&["@types/chrome"]),
            "{range} {installed}"
        );
    }
}

#[test]
fn types_packages_are_not_ambient_without_a_types_option_on_a_declared_typescript_6() {
    for range in ["^6.0.0", "~6.0.2", "6.0.2", ">=6.0.0", "^6"] {
        assert_eq!(
            unused_types(range, None, PLAIN_TSCONFIG),
            strings(&["@types/chrome"]),
            "{range}"
        );
    }
}

#[test]
fn a_types_wildcard_keeps_every_types_package_ambient_on_typescript_6() {
    for tsconfig in [
        r#"{"compilerOptions":{"types":["*"]}}"#,
        r#"{"compilerOptions":{"types":["*","node"]}}"#,
        r#"{"extends":"./base.json"}"#,
    ] {
        for installed in [None, Some("6.0.2")] {
            assert_eq!(
                unused_types("^6.0.0", installed, tsconfig),
                Vec::<String>::new(),
                "{tsconfig} {installed:?}"
            );
        }
    }
}

#[test]
fn a_member_without_typescript_follows_the_typescript_of_the_workspace_root() {
    let member = r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"@types/chrome":"0.0.1"}}"#;
    for (root_range, installed) in [("~6.0.0", None), ("latest", Some("6.0.2"))] {
        let root = format!(
            r#"{{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{{"typescript":"{root_range}"}}}}"#
        );
        let installed = installed.map(installed_typescript);
        let mut files = vec![
            ("package.json", root.as_str()),
            ("packages/app/package.json", member),
            ("packages/app/tsconfig.json", PLAIN_TSCONFIG),
            ("packages/app/src/index.ts", "export const a = 1;\n"),
        ];
        if let Some(installed) = &installed {
            files.push(("node_modules/typescript/package.json", installed.as_str()));
        }
        assert_eq!(unused_types_of(&files), strings(&["@types/chrome"]), "{root_range}");
    }
}

#[test]
fn a_typescript_range_is_read_by_the_lowest_version_it_admits() {
    // An upper bound alone admits versions below 6; a compound range is read by its lowest version.
    for range in ["<6", "<6.0.0", "<=5.9.3", ">=5 <7", "^5 || ^6", ">5.9.3 <6"] {
        assert_eq!(
            unused_types(range, None, PLAIN_TSCONFIG),
            Vec::<String>::new(),
            "{range}"
        );
    }
    for range in [">=6 <8", "^6 || ^7", ">6.0.0 <7", "6.1.0-dev.20261101"] {
        assert_eq!(
            unused_types(range, None, PLAIN_TSCONFIG),
            strings(&["@types/chrome"]),
            "{range}"
        );
    }
}

#[test]
fn a_catalog_typescript_range_is_read_from_the_workspace_catalogs() {
    let catalogs =
        "packages:\n  - packages/*\ncatalog:\n  typescript: ^6.0.0\ncatalogs:\n  legacy:\n    typescript: ~5.9.0\n";
    for (specifier, expected) in [
        ("catalog:", strings(&["@types/chrome"])),
        ("catalog:default", strings(&["@types/chrome"])),
        ("catalog:legacy", Vec::new()),
    ] {
        let member = format!(
            r#"{{"name":"app","private":true,"main":"src/index.ts","devDependencies":{{"@types/chrome":"0.0.1","typescript":"{specifier}"}}}}"#
        );
        let files = [
            ("package.json", r#"{"name":"root","private":true}"#),
            ("pnpm-workspace.yaml", catalogs),
            ("packages/app/package.json", member.as_str()),
            ("packages/app/tsconfig.json", PLAIN_TSCONFIG),
            ("packages/app/src/index.ts", "export const a = 1;\n"),
        ];
        assert_eq!(unused_types_of(&files), expected, "{specifier}");
    }
}

#[test]
fn a_peer_typescript_range_counts_only_without_another_typescript_entry() {
    for (dev, peer, expected) in [
        // The package's own entry names no version, so the peer range of its consumers does not stand in for it.
        (Some("latest"), "^6.0.0", Vec::new()),
        (Some("^5.4.0"), "^6.0.0", Vec::new()),
        (Some("^6.0.0"), "^5.0.0", strings(&["@types/chrome"])),
        (None, "^6.0.0", strings(&["@types/chrome"])),
    ] {
        let dev = dev.map_or_else(String::new, |range| format!(r#","typescript":"{range}""#));
        let manifest = format!(
            r#"{{"name":"app","private":true,"main":"src/index.ts","devDependencies":{{"@types/chrome":"0.0.1"{dev}}},"peerDependencies":{{"typescript":"{peer}"}}}}"#
        );
        let files = [
            ("package.json", manifest.as_str()),
            ("tsconfig.json", PLAIN_TSCONFIG),
            ("src/index.ts", "export const a = 1;\n"),
        ];
        assert_eq!(unused_types_of(&files), expected, "{dev} {peer}");
    }
}

#[test]
fn a_solution_tsconfig_is_judged_by_the_configs_it_references() {
    let solution = r#"{"files":[],"references":[{"path":"./tsconfig.app.json"},{"path":"./tools"}]}"#;
    let narrowing_solution =
        r#"{"files":[],"compilerOptions":{"types":[]},"references":[{"path":"./tsconfig.app.json"}]}"#;
    let node_only = r#"{"compilerOptions":{"types":["node"]}}"#;
    for (range, tsconfig, app, expected) in [
        // Vite's layout: a referenced config that includes every package keeps them on TypeScript 6.
        ("^6.0.0", solution, TYPES_EVERY, Vec::new()),
        ("^6.0.0", solution, node_only, strings(&["@types/chrome"])),
        // A referenced config without `types` follows the version.
        ("^5.4.0", narrowing_solution, PLAIN_TSCONFIG, Vec::new()),
        (
            "^6.0.0",
            narrowing_solution,
            PLAIN_TSCONFIG,
            strings(&["@types/chrome"]),
        ),
    ] {
        let configs = [
            ("tsconfig.json", tsconfig),
            ("tsconfig.app.json", app),
            ("tools/tsconfig.json", node_only),
        ];
        assert_eq!(
            unused_types_among(range, &configs),
            expected,
            "{range} {tsconfig} {app}"
        );
    }
    // A config referenced by its directory counts too.
    let configs = [
        ("tsconfig.json", solution),
        ("tsconfig.app.json", node_only),
        ("tools/tsconfig.json", TYPES_EVERY),
    ];
    assert_eq!(unused_types_among("^6.0.0", &configs), Vec::<String>::new());
}

#[test]
fn a_typescript_6_prerelease_counts_as_typescript_6() {
    assert_eq!(
        unused_types("^5.4.0", Some("6.0.0-beta"), PLAIN_TSCONFIG),
        strings(&["@types/chrome"])
    );
    assert_eq!(
        unused_types("6.1.0-dev.20261101", None, PLAIN_TSCONFIG),
        strings(&["@types/chrome"])
    );
}

#[test]
fn a_member_follows_the_nearest_typescript_install() {
    let member = r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"@types/chrome":"0.0.1"}}"#;
    let root = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
    let five = installed_typescript("5.9.3");
    let six = installed_typescript("6.0.2");
    // The member's own install, then the root's; an unreadable manifest on the way is passed over.
    for (own, at_root, expected) in [
        (Some(five.as_str()), six.as_str(), Vec::new()),
        (Some(six.as_str()), five.as_str(), strings(&["@types/chrome"])),
        (Some("{not json"), six.as_str(), strings(&["@types/chrome"])),
        (None, six.as_str(), strings(&["@types/chrome"])),
    ] {
        let mut files = vec![
            ("package.json", root),
            ("packages/app/package.json", member),
            ("packages/app/tsconfig.json", PLAIN_TSCONFIG),
            ("packages/app/src/index.ts", "export const a = 1;\n"),
            ("node_modules/typescript/package.json", at_root),
        ];
        if let Some(own) = own {
            files.push(("packages/app/node_modules/typescript/package.json", own));
        }
        assert_eq!(unused_types_of(&files), expected, "{own:?} {at_root}");
    }
}

/// The `@types` packages of the workspace of `files` that `unused-dev-dependencies` reports.
fn unused_types_of(files: &[(&str, &str)]) -> Vec<String> {
    names(&lint(files), "unused-dev-dependencies")
        .into_iter()
        .filter(|name| name.starts_with("@types/"))
        .collect()
}

/// The `unused-dev-dependencies` of a package listing `@types/chrome` and `typescript` at `range`, with the config
/// files `configs` (its `tsconfig.json` among them).
fn unused_types_among(range: &str, configs: &[(&str, &str)]) -> Vec<String> {
    let manifest = types_manifest_with(range);
    let mut files = vec![
        ("package.json", manifest.as_str()),
        ("src/index.ts", "export const a = 1;\n"),
    ];
    files.extend_from_slice(configs);
    unused_types_of(&files)
}

const TYPES_EVERY: &str = r#"{"compilerOptions":{"types":["*"]}}"#;
const TYPES_NONE: &str = r#"{"compilerOptions":{"types":[]}}"#;

#[test]
fn the_types_option_of_a_config_replaces_the_one_it_extends() {
    let chrome = strings(&["@types/chrome"]);
    for (tsconfig, base, expected) in [
        (
            r#"{"extends":"./base.json","compilerOptions":{"types":["*"]}}"#,
            TYPES_NONE,
            vec![],
        ),
        (
            r#"{"extends":"./base.json","compilerOptions":{"types":[]}}"#,
            TYPES_EVERY,
            chrome.clone(),
        ),
        (
            r#"{"extends":"./base.json","compilerOptions":{"types":["node"]}}"#,
            TYPES_EVERY,
            chrome.clone(),
        ),
        (r#"{"extends":"./base.json"}"#, TYPES_NONE, chrome.clone()),
    ] {
        assert_eq!(
            unused_types_among("^6.0.0", &[("tsconfig.json", tsconfig), ("base.json", base)]),
            expected,
            "{tsconfig} over {base}"
        );
    }
}

#[test]
fn a_later_extends_entry_overrides_an_earlier_one() {
    for (extends, expected) in [
        (r#"["./none.json","./every.json"]"#, Vec::<String>::new()),
        (r#"["./every.json","./none.json"]"#, strings(&["@types/chrome"])),
        // An entry that sets no `types` leaves the earlier one in force.
        (r#"["./every.json","./plain.json"]"#, Vec::new()),
    ] {
        let tsconfig = format!(r#"{{"extends":{extends}}}"#);
        let configs = [
            ("tsconfig.json", tsconfig.as_str()),
            ("every.json", TYPES_EVERY),
            ("none.json", TYPES_NONE),
            ("plain.json", PLAIN_TSCONFIG),
        ];
        assert_eq!(unused_types_among("^6.0.0", &configs), expected, "{extends}");
    }
}

#[test]
fn the_type_roots_of_a_config_replace_the_ones_it_extends() {
    let typings = r#"{"compilerOptions":{"typeRoots":["./typings"]}}"#;
    let default_roots = r#"{"compilerOptions":{"typeRoots":["./node_modules/@types"]}}"#;
    for (tsconfig, base, expected) in [
        (
            r#"{"extends":"./base.json","compilerOptions":{"typeRoots":["node_modules/@types","./typings"]}}"#,
            typings,
            Vec::<String>::new(),
        ),
        (
            r#"{"extends":"./base.json","compilerOptions":{"typeRoots":["./typings"]}}"#,
            default_roots,
            strings(&["@types/chrome"]),
        ),
        (r#"{"extends":"./base.json"}"#, typings, strings(&["@types/chrome"])),
    ] {
        assert_eq!(
            unused_types_among("^5.4.0", &[("tsconfig.json", tsconfig), ("base.json", base)]),
            expected,
            "{tsconfig} over {base}"
        );
    }
}

#[test]
fn declaration_files_do_not_shadow_the_runtime_file() {
    let diagnostics = lint(&[
        ("package.json", r#"{"name":"app","private":true,"main":"src/main.js"}"#),
        ("src/main.js", "import Card from './ui/Card';\nexport default Card;\n"),
        (
            "src/ui/Card.js",
            "import { helper } from './helper.js';\nexport default helper;\n",
        ),
        (
            "src/ui/Card.d.ts",
            "declare const Card: number;\nexport default Card;\n",
        ),
        ("src/ui/helper.js", "export const helper = 1;\n"),
    ]);
    assert_eq!(
        paths(&diagnostics, "unused-files"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn ignored_generated_targets_are_not_unresolved() {
    let files = [
        ("package.json", r#"{"name":"app","private":true,"main":"src/index.ts"}"#),
        (
            "src/index.ts",
            "import { A } from './generated/consts';\nimport { B } from './missing';\nexport const c = [A, B];\n",
        ),
    ];
    let without_ignore = lint(&files);
    assert_eq!(
        names(&without_ignore, "unresolved-imports").len(),
        2,
        "{without_ignore:#?}"
    );
    let mut with_ignore = files.to_vec();
    with_ignore.push((".gitignore", "src/generated\n"));
    let diagnostics = lint(&with_ignore);
    assert_eq!(
        names(&diagnostics, "unresolved-imports"),
        strings(&["./missing"]),
        "{diagnostics:#?}"
    );
}

#[test]
fn tsconfig_in_a_directory_that_is_not_a_package_applies_below_it() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"packages/app/main.js"}"#,
        ),
        (
            "packages/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@scope/*":["./*/index.ts"]}},"include":["./lib"]}"#,
        ),
        (
            "packages/app/main.js",
            "import { x } from '@scope/lib';\nexport const y = x;\n",
        ),
        ("packages/lib/index.ts", "export const x = 1;\n"),
    ]);
    assert_eq!(
        names(&diagnostics, "unresolved-imports"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unlisted-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn bundler_resolve_extensions_make_extensionless_imports_resolve() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/main.js","devDependencies":{"webpack":"5.0.0"}}"#,
        ),
        (
            "src/main.js",
            "import { page } from './page';\nexport const y = page;\n",
        ),
        ("src/page.view", "export const page = 1;\n"),
    ];
    let without = lint(&files);
    assert_eq!(names(&without, "unresolved-imports").len(), 1, "{without:#?}");
    let mut with = files.to_vec();
    with.push((
        "webpack.config.js",
        "module.exports = { entry: './src/main.js', resolve: { extensions: ['.js', '.view'] } };\n",
    ));
    let diagnostics = lint(&with);
    assert_eq!(
        names(&diagnostics, "unresolved-imports"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn root_relative_stylesheet_imports_resolve() {
    let diagnostics = lint(&[
        ("package.json", r#"{"name":"app","private":true,"main":"src/main.js"}"#),
        ("src/main.js", "import './style.css';\n"),
        ("src/style.css", "@import \"/src/base.css\";\n"),
        ("src/base.css", "body { margin: 0; }\n"),
    ]);
    assert_eq!(
        names(&diagnostics, "unresolved-imports"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        paths(&diagnostics, "unused-files"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn bazel_workspace_prefixed_requires_are_local_files() {
    let diagnostics = lint(&[
        ("package.json", r#"{"name":"app","private":true,"main":"src/main.js"}"#),
        ("MODULE.bazel", "module(name = \"mono\", version = \"1.0\")\n"),
        (
            "src/main.js",
            "const util = require('mono/lib/util');\nmodule.exports = util;\n",
        ),
        ("lib/util.js", "module.exports = 1;\n"),
    ]);
    assert_eq!(
        names(&diagnostics, "unlisted-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        paths(&diagnostics, "unused-files"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

#[test]
fn an_alias_into_an_uninstalled_node_modules_uses_the_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/main.js","dependencies":{"svelte":"4.0.0"}}"#,
        ),
        (
            "webpack.config.js",
            "const path = require('path');\nmodule.exports = { entry: './src/main.js', resolve: { alias: { svelte: path.resolve('node_modules', 'svelte') } } };\n",
        ),
        ("src/main.js", "import { onMount } from 'svelte';\nonMount(() => {});\n"),
    ]);
    assert_eq!(
        names(&diagnostics, "unresolved-imports"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// A package whose `tsconfig.json` merges its directory with its build output tree, `bazel-bin/packages/app`
/// (gitignored), where `./gen/schema.js` is generated.
const ROOT_DIRS_APP: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
    ),
    (".gitignore", "bazel-bin\n"),
    (
        "packages/app/package.json",
        r#"{"name":"@mono/app","version":"1.0.0","main":"src/index.ts","dependencies":{"uuid":"9.0.0","zod":"3.0.0"}}"#,
    ),
    (
        "packages/app/tsconfig.json",
        r#"{"compilerOptions":{"rootDirs":[".","../../bazel-bin/packages/app"]}}"#,
    ),
    (
        "packages/app/src/index.ts",
        "import { schema } from './gen/schema.js';\nexport const s = schema;\n",
    ),
    (
        "bazel-bin/packages/app/src/gen/schema.js",
        "import { z } from 'zod';\nimport { id } from './id.js';\nexport const schema = z.object({ id });\n",
    ),
    (
        "bazel-bin/packages/app/src/gen/id.js",
        "import { v4 } from 'uuid';\nexport const id = v4();\n",
    ),
];

/// [`ROOT_DIRS_APP`] with `files` added, a file of the same path replacing its own.
fn root_dirs_app(files: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    ROOT_DIRS_APP.iter().chain(files).copied().collect()
}

/// A file generated into another `rootDirs` directory (`bazel-bin`, gitignored) belongs to the package whose file
/// imports it: Node.js resolves its imports from that package's dependencies. So do the generated files beside it
/// that it imports in turn.
#[test]
fn files_reached_through_root_dirs_belong_to_the_importing_package() {
    let diagnostics = lint(ROOT_DIRS_APP);
    assert_eq!(
        names(&diagnostics, "unlisted-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unresolved-imports"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// `--trace-dependency` names the adopting package as the owner of a file reached through `rootDirs`.
#[test]
fn the_trace_of_a_dependency_names_the_package_that_adopted_the_importing_file() {
    let memory = ROOT_DIRS_APP.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(format!("/ws/{path}"), text)
    });
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(PluginSource::new(config.js.plugins.clone()))];
    let trace = JsAnalysis::build(&workspace, &config, &fs, &sources, true).trace_dependency("zod");
    let owners: Vec<(&str, &str)> = trace
        .imported_by
        .iter()
        .map(|import| (import.file.as_str(), import.package.as_str()))
        .collect();
    assert_eq!(
        owners,
        [("bazel-bin/packages/app/src/gen/schema.js", "@mono/app")],
        "{trace:#?}"
    );
    assert_eq!(trace.used_by, ["@mono/app"], "{trace:#?}");
}

/// `rootDirs` that reach into another workspace package do not take its files: a file of `packages/shared` stays
/// that package's, so its imports use the dependencies of `@mono/shared`, not of the importer.
#[test]
fn root_dirs_into_another_workspace_package_leave_its_files_with_it() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "packages/a/package.json",
            r#"{"name":"@mono/a","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        (
            "packages/a/tsconfig.json",
            r#"{"compilerOptions":{"rootDirs":["src","../shared/src"]}}"#,
        ),
        (
            "packages/a/src/index.ts",
            "import { parse } from './util.js';\nexport const p = parse;\n",
        ),
        (
            "packages/shared/package.json",
            r#"{"name":"@mono/shared","version":"1.0.0","main":"src/index.ts","dependencies":{"zod":"3.0.0"}}"#,
        ),
        ("packages/shared/src/index.ts", "export const shared = 1;\n"),
        (
            "packages/shared/src/util.ts",
            "import { z } from 'zod';\nexport const parse = z.string().parse;\n",
        ),
    ]);
    assert_eq!(
        names(&diagnostics, "unlisted-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// An adopted file passes its package on only below the root directory it was found below: a relative import from
/// app's output tree into the sibling `bazel-bin/packages/lib` reaches a file of the root, not of app.
#[test]
fn relative_imports_out_of_the_root_dir_of_an_adopted_file_are_not_adopted() {
    let diagnostics = lint(&root_dirs_app(&[
        (
            "packages/app/src/index.ts",
            "import { schema } from './gen/schema.js';\nimport { b } from './gen/other.js';\nexport const s = [schema, b];\n",
        ),
        (
            "bazel-bin/packages/app/src/gen/other.js",
            "export { b } from '../../../lib/src/b.js';\n",
        ),
        (
            "bazel-bin/packages/lib/src/b.js",
            "import { v1 } from 'uuid';\nexport const b = v1();\n",
        ),
    ]));
    assert_eq!(
        paths(&diagnostics, "unlisted-dependencies"),
        strings(&["bazel-bin/packages/lib/src/b.js"]),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unlisted-dependencies"),
        strings(&["uuid"]),
        "{diagnostics:#?}"
    );
}

/// Two packages whose `rootDirs` share one output tree reach the same generated file: the first importer, in the
/// order of the files, owns it in every run.
#[test]
fn the_first_package_to_reach_a_shared_generated_file_owns_it() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
        ),
        (".gitignore", "bazel-bin\n"),
        (
            "packages/a/package.json",
            r#"{"name":"@mono/a","version":"1.0.0","main":"src/index.ts","dependencies":{"zod":"3.0.0"}}"#,
        ),
        (
            "packages/a/tsconfig.json",
            r#"{"compilerOptions":{"rootDirs":[".","../../bazel-bin/shared"]}}"#,
        ),
        (
            "packages/a/src/index.ts",
            "import { schema } from './gen.js';\nexport const s = schema;\n",
        ),
        (
            "packages/b/package.json",
            r#"{"name":"@mono/b","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        (
            "packages/b/tsconfig.json",
            r#"{"compilerOptions":{"rootDirs":[".","../../bazel-bin/shared"]}}"#,
        ),
        (
            "packages/b/src/index.ts",
            "import { schema } from './gen.js';\nexport const s = schema;\n",
        ),
        (
            "bazel-bin/shared/src/gen.js",
            "import { z } from 'zod';\nexport const schema = z.object({});\n",
        ),
    ]);
    assert_eq!(
        names(&diagnostics, "unlisted-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-dependencies"),
        Vec::<String>::new(),
        "{diagnostics:#?}"
    );
}

/// A `rules_js` package whose `BUILD.bazel` declares `src/gen/schema.js` as the output of a `js_run_binary` target,
/// generated into `bazel-bin` (gitignored), and whose `src/index.ts` imports it and a file nothing declares.
const BAZEL_APP: &[(&str, &str)] = &[
    (
        "MODULE.bazel",
        "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
    ),
    (".gitignore", "bazel-*\n"),
    (
        "package.json",
        r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
    ),
    (
        "packages/app/package.json",
        r#"{"name":"@mono/app","version":"1.0.0","main":"src/index.ts","dependencies":{"zod":"3.0.0"}}"#,
    ),
    (
        "packages/app/BUILD.bazel",
        concat!(
            "load(\"@aspect_rules_js//js:defs.bzl\", \"js_run_binary\")\n\n",
            "npm_link_all_packages(name = \"node_modules\")\n\n",
            "js_run_binary(\n",
            "    name = \"schema\",\n",
            "    tool = \"//tools:codegen\",\n",
            "    srcs = [\":node_modules/zod\"],\n",
            "    outs = [\"src/gen/schema.js\"],\n",
            ")\n",
        ),
    ),
    (
        "packages/app/src/index.ts",
        "import { schema } from './gen/schema.js';\nimport { typo } from './gen/typo.js';\nexport const s = [schema, typo];\n",
    ),
];

const BAZEL_ROOT_DIRS: (&str, &str) = (
    "packages/app/tsconfig.json",
    r#"{"compilerOptions":{"rootDirs":[".","../../bazel-bin/packages/app"]}}"#,
);

/// The generated file as the last build left it, importing a package that `@mono/app` does not list.
const BAZEL_BUILT: (&str, &str) = (
    "bazel-bin/packages/app/src/gen/schema.js",
    "import { v4 } from 'uuid';\nexport const schema = v4();\n",
);

/// Another output of a build that did not build `:schema`.
const BAZEL_PARTIAL: (&str, &str) = ("bazel-bin/packages/app/src/other.js", "export const other = 1;\n");

/// The rule, path and message of every diagnostic, sorted.
fn findings(diagnostics: &[Diagnostic]) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .map(|d| {
            format!(
                "{} {}: {}",
                d.rule.as_str(),
                d.location.path.to_string_lossy().replace('\\', "/"),
                d.message
            )
        })
        .collect();
    found.sort();
    found
}

/// In a Bazel repository an import of an output a `BUILD` file declares is generated code, whether or not `rootDirs`
/// reach `bazel-bin` and whatever the last build left there: the results are the same, the declared output is never
/// unresolved, the copy in `bazel-bin` is never read, and an import that no `BUILD` file declares stays unresolved.
#[test]
fn bazel_outputs_are_generated_whatever_was_built() {
    let variants: [&[(&str, &str)]; 6] = [
        &[],
        &[BAZEL_ROOT_DIRS],
        &[BAZEL_ROOT_DIRS, BAZEL_BUILT],
        &[BAZEL_ROOT_DIRS, BAZEL_PARTIAL],
        &[BAZEL_BUILT],
        &[BAZEL_PARTIAL],
    ];
    let mut baseline: Option<Vec<String>> = None;
    for extra in variants {
        let files: Vec<(&str, &str)> = BAZEL_APP.iter().chain(extra).copied().collect();
        let diagnostics = lint(&files);
        assert_eq!(
            paths(&diagnostics, "unresolved-imports"),
            strings(&["packages/app/src/index.ts"]),
            "{extra:?}: {diagnostics:#?}"
        );
        let unresolved: Vec<&str> = diagnostics
            .iter()
            .filter(|d| d.rule.as_str() == "unresolved-imports")
            .map(|d| d.message.as_str())
            .collect();
        assert!(
            unresolved.len() == 1 && unresolved[0].contains("./gen/typo.js"),
            "{extra:?}: {unresolved:?}"
        );
        assert_eq!(
            names(&diagnostics, "unlisted-dependencies"),
            Vec::<String>::new(),
            "{extra:?}: {diagnostics:#?}"
        );
        assert_eq!(
            names(&diagnostics, "unused-dependencies"),
            Vec::<String>::new(),
            "{extra:?}: {diagnostics:#?}"
        );
        let found = findings(&diagnostics);
        assert!(
            found.iter().all(|line| !line.contains(" bazel-")),
            "{extra:?}: {found:#?}"
        );
        match &baseline {
            Some(first) => assert_eq!(&found, first, "{extra:?}"),
            None => baseline = Some(found),
        }
    }
}

/// A `rules_js` package whose `ts_project` builds from `a.ts` and `b.ts`, which imports `c.ts`, and whose `js_binary`
/// runs `tool.mjs`; `orphan.ts` is named by nothing.
const BAZEL_SOURCES: &[(&str, &str)] = &[
    (
        "MODULE.bazel",
        "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
    ),
    ("package.json", r#"{"name":"app","version":"1.0.0","private":true}"#),
    (
        "BUILD.bazel",
        concat!(
            "load(\"@aspect_rules_js//js:defs.bzl\", \"js_binary\")\n",
            "load(\"@aspect_rules_ts//ts:defs.bzl\", \"ts_project\")\n\n",
            "ts_project(name = \"lib\", srcs = [\"a.ts\", \"b.ts\"])\n\n",
            "js_binary(name = \"tool\", entry_point = \"tool.mjs\")\n",
        ),
    ),
    ("a.ts", "export const unusedInA = 1;\nexport type UnusedInA = string;\n"),
    ("b.ts", "import { helper } from './c';\nhelper();\n"),
    ("c.ts", "export function helper(): void {}\n"),
    ("tool.mjs", "export const toolOption = 1;\n"),
    ("orphan.ts", "export const lonely = 1;\n"),
];

/// The `srcs` of a `ts_project` are used files, and so is what they import, but they are no entries: their unused
/// exports and types are reported as any module's. The `entry_point` of a `js_binary` is an entry, whose exports are
/// public.
#[test]
fn bazel_sources_are_used_and_their_exports_judged() {
    let diagnostics = lint(BAZEL_SOURCES);
    assert_eq!(
        paths(&diagnostics, "unused-files"),
        strings(&["orphan.ts"]),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-exports"),
        strings(&["unusedInA"]),
        "{diagnostics:#?}"
    );
    assert_eq!(
        paths(&diagnostics, "unused-exports"),
        strings(&["a.ts"]),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-types"),
        strings(&["UnusedInA"]),
        "{diagnostics:#?}"
    );
}

/// The root file of a Bazel repository that uses `rules_js`.
const RULES_JS_MODULE: (&str, &str) = (
    "MODULE.bazel",
    "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
);

/// A file that a `ts_project` builds from and that an entry rule also matches (the `main` of `package.json`, a default
/// entry, an `entry` glob of the configuration) stays an entry, whose exports are public; the other source beside it,
/// `spare.ts`, is a build source, whose export is judged.
#[test]
fn a_bazel_source_that_is_also_an_entry_keeps_public_exports() {
    let without_main = r#"{"name":"app","version":"1.0.0","private":true}"#;
    for (case, manifest, toml, entry, spare) in [
        (
            "main",
            r#"{"name":"app","version":"1.0.0","private":true,"main":"app/main.ts"}"#,
            "",
            "app/main.ts",
            "app/spare.ts",
        ),
        ("default entry", without_main, "", "src/index.ts", "src/spare.ts"),
        (
            "entry glob",
            without_main,
            "[js]\nentry = [\"app/cli.ts\"]\n",
            "app/cli.ts",
            "app/spare.ts",
        ),
    ] {
        let build = format!("ts_project(name = \"lib\", srcs = [\"{entry}\", \"{spare}\"])\n");
        let files = [
            RULES_JS_MODULE,
            ("package.json", manifest),
            ("BUILD.bazel", build.as_str()),
            (entry, "export const api = 1;\n"),
            (spare, "export const spare = 1;\n"),
        ];
        let diagnostics = lint_with(&files, toml);
        assert_eq!(
            paths(&diagnostics, "unused-files"),
            Vec::<String>::new(),
            "{case}: {diagnostics:#?}"
        );
        assert_eq!(
            names(&diagnostics, "unused-exports"),
            strings(&["spare"]),
            "{case}: {diagnostics:#?}"
        );
        assert_eq!(
            paths(&diagnostics, "unused-exports"),
            strings(&[spare]),
            "{case}: {diagnostics:#?}"
        );
    }
}

/// A `rules_js` package whose `js_binary` runs `tool.mjs` with `assets.ts` and whose `js_test` runs `check.test.mjs`
/// with `golden.ts`.
const BAZEL_TEST_DATA: &[(&str, &str)] = &[
    RULES_JS_MODULE,
    ("package.json", r#"{"name":"app","version":"1.0.0","private":true}"#),
    (
        "BUILD.bazel",
        concat!(
            "js_binary(name = \"tool\", entry_point = \"tool.mjs\", data = [\"assets.ts\"])\n\n",
            "js_test(name = \"check\", entry_point = \"check.test.mjs\", data = [\"golden.ts\"])\n",
        ),
    ),
    ("tool.mjs", "console.log(1);\n"),
    ("assets.ts", "export const asset = 1;\n"),
    ("check.test.mjs", "console.log(2);\n"),
    ("golden.ts", "export const golden = 1;\n"),
];

/// The `data` of a `js_test` is a build source of the tests only: a production run leaves it unused, while the `data`
/// of a `js_binary` stays a used production build source whose export is judged.
#[test]
fn bazel_test_data_is_no_production_build_source() {
    let development = lint(BAZEL_TEST_DATA);
    assert_eq!(
        paths(&development, "unused-files"),
        Vec::<String>::new(),
        "{development:#?}"
    );
    assert_eq!(
        names(&development, "unused-exports"),
        strings(&["asset", "golden"]),
        "{development:#?}"
    );
    let production = lint_with(BAZEL_TEST_DATA, "[js]\nproduction = true\n");
    assert_eq!(
        paths(&production, "unused-files"),
        strings(&["golden.ts"]),
        "{production:#?}"
    );
    assert_eq!(
        names(&production, "unused-exports"),
        strings(&["asset"]),
        "{production:#?}"
    );
}

/// `duplicate-exports` and the enum and namespace member rules judge a build source, which an entry would exempt:
/// `b.ts` reads `Color.Red`, `Shapes.circle`, `used` and `duplicate` of `a.ts`, both in the `srcs` of a `ts_project`.
#[test]
fn duplicates_and_members_of_a_bazel_source_are_reported() {
    let diagnostics = lint(&[
        RULES_JS_MODULE,
        ("package.json", r#"{"name":"app","version":"1.0.0","private":true}"#),
        (
            "BUILD.bazel",
            "ts_project(name = \"lib\", srcs = [\"a.ts\", \"b.ts\"])\n",
        ),
        (
            "a.ts",
            concat!(
                "export enum Color {\n  Red,\n  Blue,\n}\n",
                "export namespace Shapes {\n  export const circle = 1;\n  export const line = 2;\n}\n",
                "export const used = 1;\nexport const duplicate = used;\n",
            ),
        ),
        (
            "b.ts",
            concat!(
                "import { Color, Shapes, used, duplicate } from './a';\n",
                "console.log(Color.Red, Shapes.circle, used, duplicate);\n",
            ),
        ),
    ]);
    for (rule, expected) in [
        ("duplicate-exports", "used"),
        ("unused-enum-members", "Color.Blue"),
        ("unused-namespace-members", "Shapes.line"),
    ] {
        assert_eq!(
            names(&diagnostics, rule),
            strings(&[expected]),
            "{rule}: {diagnostics:#?}"
        );
        assert_eq!(
            paths(&diagnostics, rule),
            strings(&["a.ts"]),
            "{rule}: {diagnostics:#?}"
        );
    }
}

/// A root `BUILD` file whose `ts_project` names a file of the workspace member `packages/foo`, which has no `BUILD`
/// file of its own, makes it a build source of that member: it and what it imports are used and its export is judged,
/// while `orphan.ts`, which nothing names, is unused.
#[test]
fn a_root_bazel_source_in_a_member_is_a_build_source() {
    let diagnostics = lint(&[
        RULES_JS_MODULE,
        (
            "package.json",
            r#"{"name":"root","version":"1.0.0","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "BUILD.bazel",
            "ts_project(name = \"foo\", srcs = [\"packages/foo/src/x.ts\"])\n",
        ),
        (
            "packages/foo/package.json",
            r#"{"name":"foo","version":"1.0.0","private":true}"#,
        ),
        (
            "packages/foo/src/x.ts",
            "import { help } from './y';\nhelp();\nexport const spare = 1;\n",
        ),
        ("packages/foo/src/y.ts", "export function help(): void {}\n"),
        ("packages/foo/src/orphan.ts", "export const lonely = 1;\n"),
    ]);
    assert_eq!(
        paths(&diagnostics, "unused-files"),
        strings(&["packages/foo/src/orphan.ts"]),
        "{diagnostics:#?}"
    );
    assert_eq!(
        names(&diagnostics, "unused-exports"),
        strings(&["spare"]),
        "{diagnostics:#?}"
    );
    assert_eq!(
        paths(&diagnostics, "unused-exports"),
        strings(&["packages/foo/src/x.ts"]),
        "{diagnostics:#?}"
    );
}

/// A root `BUILD` file naming a file of a project of its own below the root (a nested root) lists it as a build source
/// of another project, for a multi-root host to hand to that root as a build source: not among the files of other
/// projects, which that root takes as entries.
#[test]
fn a_root_bazel_source_in_a_nested_root_is_a_build_source_of_another_project() {
    let mut files = vec![
        ("/ws/.git/HEAD", ""),
        ("/ws/MODULE.bazel", RULES_JS_MODULE.1),
        (
            "/ws/package.json",
            r#"{"name":"outer","version":"0.0.0","private":true,"main":"index.js"}"#,
        ),
        ("/ws/index.js", "console.log(1);\n"),
        (
            "/ws/BUILD.bazel",
            "ts_project(name = \"tools\", srcs = [\"tools/lib/x.ts\"])\n",
        ),
    ];
    files.extend(NESTED_TOOLS);
    let report = report_at("/ws", &files);
    assert_eq!(report.other_project_build_sources, [PathBuf::from("/ws/tools/lib/x.ts")]);
    assert_eq!(report.other_project_files, Vec::<PathBuf>::new());
}

/// The files of a nested root `tools` below `/ws`: `x.ts` imports `y.ts` and exports `spare`, which nothing imports,
/// and nothing uses `orphan.ts`.
const NESTED_TOOLS: [(&str, &str); 4] = [
    (
        "/ws/tools/package.json",
        r#"{"name":"tools","version":"0.0.0","private":true}"#,
    ),
    (
        "/ws/tools/lib/x.ts",
        "import { help } from './y';\nhelp();\nexport const spare = 1;\n",
    ),
    ("/ws/tools/lib/y.ts", "export function help(): void {}\n"),
    ("/ws/tools/lib/orphan.ts", "export const lonely = 1;\n"),
];

/// A build source another root hands this one is used and its imports are followed, but its exports are judged, as
/// for a build source of its own; the same file handed as an entry has public exports.
#[test]
fn a_build_source_from_another_root_is_used_and_its_exports_are_judged() {
    let mut files = vec![("/ws/.git/HEAD", "")];
    files.extend(NESTED_TOOLS);
    let handed = [PathBuf::from("lib/x.ts")];

    let report = report_with("/ws/tools", &files, &RunOptions::default().with_extra_build_sources(&handed));
    assert_eq!(
        paths(&report.diagnostics, "unused-files"),
        strings(&["lib/orphan.ts"]),
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(names(&report.diagnostics, "unused-exports"), strings(&["spare"]), "{:#?}", report.diagnostics);
    assert_eq!(
        paths(&report.diagnostics, "unused-exports"),
        strings(&["lib/x.ts"]),
        "{:#?}",
        report.diagnostics
    );

    let report = report_with("/ws/tools", &files, &RunOptions::default().with_extra_entries(&handed));
    assert_eq!(
        paths(&report.diagnostics, "unused-files"),
        strings(&["lib/orphan.ts"]),
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(names(&report.diagnostics, "unused-exports"), Vec::<String>::new(), "{:#?}", report.diagnostics);
}

/// Outside a Bazel repository nothing changes: the output a `BUILD` file declares is unresolved like any missing file.
#[test]
fn build_files_declare_nothing_outside_a_bazel_repository() {
    let files: Vec<(&str, &str)> = BAZEL_APP
        .iter()
        .filter(|(path, _)| *path != "MODULE.bazel")
        .copied()
        .collect();
    let diagnostics = lint(&files);
    let mut unresolved: Vec<&str> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "unresolved-imports")
        .map(|d| d.message.as_str())
        .collect();
    unresolved.sort_unstable();
    assert_eq!(unresolved.len(), 2, "{diagnostics:#?}");
    assert!(
        unresolved.iter().any(|message| message.contains("./gen/schema.js")),
        "{unresolved:?}"
    );
}

/// A `BUILD` file of the nested package `adev/shared-docs` whose labels name the `node_modules` of an enclosing
/// package: `label` in the `deps` of a `js_library` there. Both packages and the root list `diff`.
fn bazel_foreign_label(label: &str) -> Vec<(String, String)> {
    let build = format!(
        "load(\"@aspect_rules_js//js:defs.bzl\", \"js_library\")\n\
         js_library(name = \"pipeline\", srcs = [\"main.js\"], deps = [\"{label}\"])\n"
    );
    [
        ("MODULE.bazel", "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n"),
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["adev","adev/shared-docs"],"dependencies":{"diff":"9.0.0"}}"#,
        ),
        ("BUILD.bazel", "exports_files([])\n"),
        (
            "adev/package.json",
            r#"{"name":"adev","version":"1.0.0","private":true,"main":"main.js","dependencies":{"diff":"9.0.0"}}"#,
        ),
        ("adev/BUILD.bazel", "js_library(name = \"adev\", srcs = [\"main.js\"])\n"),
        ("adev/main.js", "module.exports = 3;\n"),
        (
            "adev/shared-docs/package.json",
            r#"{"name":"shared-docs","version":"1.0.0","private":true,"main":"index.js","dependencies":{"diff":"9.0.0"}}"#,
        ),
        ("adev/shared-docs/BUILD.bazel", "exports_files([\"package.json\"])\n"),
        ("adev/shared-docs/index.js", "module.exports = 1;\n"),
        ("adev/shared-docs/pipeline/BUILD.bazel", build.as_str()),
        ("adev/shared-docs/pipeline/main.js", "module.exports = 2;\n"),
    ]
    .iter()
    .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
    .collect()
}

/// A label of an enclosing package's `node_modules` uses the dependency of the package it names, and of no other: the
/// nested package that owns the `BUILD` file does not use its own listing through it.
#[test]
fn a_label_of_an_enclosing_packages_node_modules_credits_only_that_package() {
    for (label, unused) in [
        (
            "//adev:node_modules/diff",
            &["adev/shared-docs/package.json", "package.json"][..],
        ),
        (
            "//:node_modules/diff",
            &["adev/package.json", "adev/shared-docs/package.json"][..],
        ),
    ] {
        let files = bazel_foreign_label(label);
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        let diagnostics = lint(&borrowed);
        assert_eq!(
            paths(&diagnostics, "unused-dependencies"),
            strings(unused),
            "{label}: {diagnostics:#?}"
        );
        assert_eq!(
            names(&diagnostics, "unlisted-dependencies"),
            Vec::<String>::new(),
            "{label}: {diagnostics:#?}"
        );
    }
}
