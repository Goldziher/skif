//! False positives of the dependency and resolution rules: binaries from ancestor projects, lockfile facts, on-demand
//! tools, ambient `@types`, declaration files, generated targets and root-relative paths.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs, Report};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run, run_unsorted};
use skif_workspace::Workspace;

fn lint_at(root: &str, files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files
        .iter()
        .fold(MemoryFs::new(), |memory, (path, text)| memory.with_file(path, text));
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(root), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// The sorted report of the workspace at `root`, with the files and extra entries a multi-root host reads.
fn report_at(root: &str, files: &[(&str, &str)]) -> Report {
    let memory = files
        .iter()
        .fold(MemoryFs::new(), |memory, (path, text)| memory.with_file(path, text));
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(root), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let rooted: Vec<(String, &str)> = files
        .iter()
        .map(|(path, text)| (format!("/ws/{path}"), *text))
        .collect();
    let borrowed: Vec<(&str, &str)> = rooted.iter().map(|(path, text)| (path.as_str(), *text)).collect();
    lint_at("/ws", &borrowed)
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
        assert_eq!(unused_types(range, None, PLAIN_TSCONFIG), Vec::<String>::new(), "{range}");
    }
    for range in [">=6 <8", "^6 || ^7", ">6.0.0 <7", "6.1.0-dev.20261101"] {
        assert_eq!(unused_types(range, None, PLAIN_TSCONFIG), strings(&["@types/chrome"]), "{range}");
    }
}

#[test]
fn a_catalog_typescript_range_is_read_from_the_workspace_catalogs() {
    let catalogs = "packages:\n  - packages/*\ncatalog:\n  typescript: ^6.0.0\ncatalogs:\n  legacy:\n    typescript: ~5.9.0\n";
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
        ("^6.0.0", narrowing_solution, PLAIN_TSCONFIG, strings(&["@types/chrome"])),
    ] {
        let configs = [
            ("tsconfig.json", tsconfig),
            ("tsconfig.app.json", app),
            ("tools/tsconfig.json", node_only),
        ];
        assert_eq!(unused_types_among(range, &configs), expected, "{range} {tsconfig} {app}");
    }
    // A config referenced by its directory counts too.
    let configs = [
        ("tsconfig.json", solution),
        ("tsconfig.app.json", node_only),
        ("tools/tsconfig.json", TYPES_EVERY),
    ];
    assert_eq!(unused_types_among("^6.0.0", &configs), Vec::<String>::new());
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
        (r#"{"extends":"./base.json","compilerOptions":{"types":["*"]}}"#, TYPES_NONE, vec![]),
        (r#"{"extends":"./base.json","compilerOptions":{"types":[]}}"#, TYPES_EVERY, chrome.clone()),
        (r#"{"extends":"./base.json","compilerOptions":{"types":["node"]}}"#, TYPES_EVERY, chrome.clone()),
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
