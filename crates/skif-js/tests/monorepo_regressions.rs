//! Regressions found on a large pnpm monorepo: builtin-named packages, HTML entries of the vitest plugin, referenced
//! `tsconfig` paths, extglob entries, shared config helpers, stylesheet lookups and per-package bundler extensions.
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

/// Lints `memory` with the JS provider and the plugin table, the way the CLI does.
fn lint_memory(memory: MemoryFs, toml: &str) -> Vec<Diagnostic> {
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (a.rule.as_str(), &a.location.path, &a.message).cmp(&(b.rule.as_str(), &b.location.path, &b.message))
    });
    diagnostics
}

fn lint_files(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    lint_memory(memory, toml)
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            let name = d.dependency.clone().unwrap_or_else(|| {
                let start = d.message.find('`').map_or(0, |i| i + 1);
                let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
                d.message[start..end].to_owned()
            });
            format!("{}: {name}", d.location.path.display())
        })
        .collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

const PNPM: &str = r#""packageManager":"pnpm@9.0.0""#;

fn manifest(body: &str) -> String {
    format!(r"{{{body}}}")
}

#[test]
fn trailing_slash_imports_name_npm_packages_not_builtins() {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"main":"index.js","dependencies":{{"buffer":"6.0.3","events":"3.3.0"}}"#
    ));
    let diagnostics = lint_files(
        &[
            ("package.json", &package),
            (
                "index.js",
                "import buf from 'buffer/';\nimport ev from 'events/';\nexport default [buf, ev];\n",
            ),
        ],
        "",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn bare_builtin_imports_use_a_listed_package_of_that_name() {
    // A bundler resolves `events` to the listed polyfill for the browser; `node:util` is only ever the builtin.
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"main":"index.js","dependencies":{{"events":"3.3.0","util":"0.12.5"}}"#
    ));
    let diagnostics = lint_files(
        &[
            ("package.json", &package),
            (
                "index.js",
                "import { EventEmitter } from 'events';\nimport { format } from 'node:util';\nexport default [EventEmitter, format];\n",
            ),
        ],
        "",
    );
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["package.json: util"],
        "{diagnostics:#?}"
    );
}

/// A project with a `.pnpmfile.cjs` that requires `semver`, the files `source` and a manifest listing `dependencies`;
/// `src/index.js` is its `main` when `source` has it.
fn pnpmfile_project(source: &[(&'static str, &'static str)], dependencies: &str) -> Vec<Diagnostic> {
    let main = if source.iter().any(|(path, _)| *path == "src/index.js") {
        r#""main":"src/index.js","#
    } else {
        ""
    };
    let package = manifest(&format!(
        r#""name":"remote","version":"1.0.0","private":true,{PNPM},{main}"dependencies":{{{dependencies}}}"#
    ));
    let mut files = vec![
        ("package.json", package.as_str()),
        ("pnpm-workspace.yaml", "pnpmfile: .pnpmfile.cjs\n"),
        (
            ".pnpmfile.cjs",
            "const semver = require('semver');\nmodule.exports = { hooks: { readPackage: (pkg) => (semver.valid(pkg.version) ? pkg : pkg) } };\n",
        ),
    ];
    files.extend_from_slice(source);
    lint_files(&files, "")
}

#[test]
fn a_pnpmfile_alone_does_not_give_a_project_source_whose_dependencies_are_judged() {
    // vscode's `remote`: the manifest lists what the server installs, and the pnpm hook is its only JavaScript. Were
    // the hook project code, `express` would be unused.
    let diagnostics = pnpmfile_project(&[], r#""express":"4.21.0","semver":"7.6.0""#);
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn a_pnpmfile_uses_what_it_imports_in_a_project_with_source() {
    let diagnostics = pnpmfile_project(
        &[(
            "src/index.js",
            "import express from 'express';\nexport default express;\n",
        )],
        r#""express":"4.21.0","lodash":"4.17.21","semver":"7.6.0""#,
    );
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["package.json: lodash"],
        "{diagnostics:#?}"
    );
}

#[test]
fn vitest_adds_index_html_scripts_without_the_vite_plugin() {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"scripts":{{"test":"vitest run"}},"dependencies":{{"lib-a":"1.0.0"}},"devDependencies":{{"vitest":"2.0.0"}}"#
    ));
    let files = [
        ("package.json", package.as_str()),
        (
            "vitest.config.ts",
            "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { include: ['src/**/*.test.ts'] } });\n",
        ),
        (
            "index.html",
            "<html><body><script type=\"module\" src=\"src/boot.js\"></script></body></html>",
        ),
        ("src/boot.js", "import 'lib-a';\n"),
        ("src/a.test.ts", "import { it } from 'vitest';\nit('x', () => {});\n"),
    ];
    let diagnostics = lint_files(&files, "[js.plugins]\nvite = false\n");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
}

fn references_workspace(app_manifest: &str) -> Vec<(&'static str, String)> {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"devDependencies":{{"typescript":"5.6.0"}}"#
    ));
    let host = manifest(r#""name":"host","version":"1.0.0","private":true,"scripts":{"build":"node src/index.js"}"#);
    let icons = manifest(
        r#""name":"@scope/icons","version":"0.0.0","private":true,"type":"module","main":"./dist/index.js","types":"./dist/index.d.ts","exports":{".":{"types":"./dist/index.d.ts","import":"./dist/index.js"}}"#,
    );
    vec![
        ("package.json", root),
        (
            "pnpm-workspace.yaml",
            "packages:\n  - app\n  - host\n  - icons\n".to_owned(),
        ),
        (
            "tsconfig.json",
            r#"{"files":[],"references":[{"path":"./host"}]}"#.to_owned(),
        ),
        ("host/package.json", host),
        (
            "host/tsconfig.json",
            r#"{"compilerOptions":{"composite":true,"paths":{"@scope/icons":["../icons"]}}}"#.to_owned(),
        ),
        ("host/src/index.js", "console.log(1);\n".to_owned()),
        ("icons/package.json", icons),
        ("app/package.json", app_manifest.to_owned()),
        (
            "app/src/index.js",
            "import { Icon } from '@scope/icons';\nconsole.log(Icon);\n".to_owned(),
        ),
    ]
}

fn lint_owned(files: &[(&str, String)]) -> Vec<Diagnostic> {
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
    lint_files(&borrowed, "")
}

#[test]
#[ignore = "the unlisted case still resolves through the referenced paths"]
fn referenced_tsconfig_paths_do_not_become_aliases_of_other_packages() {
    let listed = manifest(
        r#""name":"app","version":"1.0.0","private":true,"scripts":{"build":"node src/index.js"},"dependencies":{"@scope/icons":"workspace:*"}"#,
    );
    let diagnostics = lint_owned(&references_workspace(&listed));
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");

    let unlisted = manifest(r#""name":"app","version":"1.0.0","private":true,"scripts":{"build":"node src/index.js"}"#);
    let diagnostics = lint_owned(&references_workspace(&unlisted));
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["app/src/index.js: @scope/icons"],
        "{diagnostics:#?}"
    );
}

#[test]
fn extglob_groups_in_a_config_entry_are_entries() {
    let package = manifest(&format!(
        r#""name":"lib","version":"1.0.0","private":true,{PNPM},"scripts":{{"build":"tsup"}},"dependencies":{{"dep-a":"1.0.0","dep-b":"1.0.0"}},"devDependencies":{{"tsup":"8.0.0"}}"#
    ));
    let files = [
        ("package.json", package.as_str()),
        (
            "tsup.config.ts",
            "import { defineConfig } from 'tsup';\nexport default defineConfig({ entry: ['src/**/*.(ts|tsx)'], format: ['esm'] });\n",
        ),
        ("src/a.ts", "import 'dep-a';\n"),
        ("src/b.tsx", "import 'dep-b';\n"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
}

#[test]
#[ignore = "the helper's package is not credited yet"]
fn a_shared_config_helper_in_a_sibling_package_credits_the_files_it_names() {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"devDependencies":{{"vitest":"2.0.0","mem-db":"1.0.0"}}"#
    ));
    let cfg = manifest(
        r#""name":"cfg","version":"1.0.0","private":true,"devDependencies":{"vitest":"2.0.0","@scope/env":"workspace:*"}"#,
    );
    let env = manifest(r#""name":"@scope/env","version":"1.0.0","private":true,"main":"./index.js""#);
    let lib = manifest(r#""name":"lib","version":"1.0.0","private":true,"devDependencies":{"vitest":"2.0.0"}"#);
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - cfg\n  - lib\n  - env\n"),
        ("cfg/package.json", cfg.as_str()),
        (
            "cfg/vitest.ts",
            "import path from 'node:path';\nimport { defineConfig } from 'vitest/config';\nconst here = import.meta.dirname;\nexport function defineSharedConfig(options: { db?: boolean } = {}) {\n  return defineConfig({ test: { globalSetup: options.db ? [path.join(here, 'global-setup.ts')] : [] } });\n}\n",
        ),
        (
            "cfg/global-setup.ts",
            "import { MemDb } from 'mem-db';\nimport { env } from '@scope/env';\nexport default async function setup() { return new MemDb(env); }\n",
        ),
        ("env/package.json", env.as_str()),
        ("env/index.js", "export const env = {};\n"),
        ("lib/package.json", lib.as_str()),
        (
            "lib/vitest.config.ts",
            "import { defineSharedConfig } from '../cfg/vitest';\nexport default defineSharedConfig({ db: true });\n",
        ),
        ("lib/src/index.ts", "export const x = 1;\n"),
    ];
    let diagnostics = lint_files(&files, "");
    for rule in ["unused-files", "unused-dependencies", "unused-dev-dependencies"] {
        assert_eq!(found(&diagnostics, rule), none(), "{rule}: {diagnostics:#?}");
    }
}

fn stylesheet_workspace(partial: &str, tsconfig: &str) -> Vec<(String, String)> {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"scripts":{{"build":"node src/index.js"}}"#
    ));
    vec![
        ("package.json".to_owned(), package),
        ("tsconfig.json".to_owned(), tsconfig.to_owned()),
        (
            "src/index.js".to_owned(),
            "import './views/deep/deep.scss';\nimport './index.scss';\n".to_owned(),
        ),
        ("src/index.scss".to_owned(), "@import 'components/mixins';\n".to_owned()),
        (
            "src/views/deep/deep.scss".to_owned(),
            "@import 'components/mixins';\n".to_owned(),
        ),
        (
            format!("src/components/{partial}"),
            "@mixin m { color: red; }\n".to_owned(),
        ),
    ]
}

fn lint_strings(files: &[(String, String)], toml: &str) -> Vec<Diagnostic> {
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    lint_files(&borrowed, toml)
}

#[test]
fn stylesheet_imports_resolve_through_base_url_with_stylesheet_rules() {
    for partial in ["mixins.scss", "_mixins.scss"] {
        let files = stylesheet_workspace(partial, r#"{"compilerOptions":{"baseUrl":"src"}}"#);
        let diagnostics = lint_strings(&files, "");
        assert_eq!(
            found(&diagnostics, "unlisted-dependencies"),
            none(),
            "{partial}: {diagnostics:#?}"
        );
        assert_eq!(
            found(&diagnostics, "unused-files"),
            none(),
            "{partial}: {diagnostics:#?}"
        );
    }
}

#[test]
fn stylesheet_imports_resolve_through_configured_paths() {
    let files = stylesheet_workspace("mixins.scss", "{}");
    let toml = "[js.workspaces.\".\".paths]\n\"components/*\" = [\"./src/components/*\"]\n";
    let diagnostics = lint_strings(&files, toml);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn bundler_extensions_apply_to_the_package_that_declares_them() {
    let root = manifest(&format!(r#""name":"root","private":true,{PNPM}"#));
    let bundled = manifest(
        r#""name":"bundled","version":"1.0.0","private":true,"scripts":{"build":"vite build"},"devDependencies":{"vite":"6.0.0"}"#,
    );
    let other = manifest(r#""name":"other","version":"1.0.0","private":true,"main":"src/index.js""#);
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - bundled\n  - other\n"),
        ("bundled/package.json", bundled.as_str()),
        (
            "bundled/vite.config.js",
            "import { defineConfig } from 'vite';\nexport default defineConfig({ resolve: { extensions: ['.js', '.svg'] } });\n",
        ),
        ("other/package.json", other.as_str()),
        (
            "other/src/index.js",
            "import Logo from './logo';\nexport default Logo;\n",
        ),
        ("other/src/logo.svg", "<svg/>"),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        ["other/src/index.js: ./logo"],
        "{diagnostics:#?}"
    );
}

#[test]
fn a_script_path_that_leaves_its_package_is_the_file_it_names() {
    let root = manifest(&format!(
        r#""name":"root","version":"1.0.0","private":true,{PNPM},"devDependencies":{{"lodash":"4.17.21"}}"#
    ));
    let db = manifest(
        r#""name":"db","version":"1.0.0","private":true,"scripts":{"seed":"tsx ../../scripts/seed.ts"},"devDependencies":{"tsx":"4.0.0"}"#,
    );
    let diagnostics = lint_files(
        &[
            ("package.json", &root),
            ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
            ("packages/db/package.json", &db),
            ("scripts/seed.ts", "import { noop } from 'lodash';\nnoop();\n"),
        ],
        "",
    );
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
}

#[test]
fn a_url_into_node_modules_is_a_path_not_a_package() {
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"main":"src/index.js""#
    ));
    let diagnostics = lint_files(
        &[
            ("package.json", &package),
            (
                "src/index.js",
                "export const tsx = new URL('../node_modules/.bin/tsx', import.meta.url);\n\
                 export const later = new URL('./node_modules/later', import.meta.url);\n",
            ),
            ("node_modules/.bin/tsx", "#!/usr/bin/env node\n"),
        ],
        "",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A package reached from `src/main.ts` only, where `src/role-selector.ts` imports exports of `src/store.ts` but no
/// entry reaches it. `published` is the `main` of the manifest.
fn incomplete_entries_workspace(published: &str) -> Vec<(&'static str, String)> {
    let package = manifest(&format!(
        r#""name":"web","version":"1.0.0","private":true,{PNPM},"main":"{published}","scripts":{{"start":"node src/main.ts"}}"#
    ));
    vec![
        ("package.json", package),
        (
            "src/main.ts",
            "import { createStore, Level, Paths } from './store';\nimport * as routes from './routes';\n\
             const route: routes.Route = Paths.home;\ncreateStore(Level.Low, routes.list, route);\n"
                .to_owned(),
        ),
        (
            "src/store.ts",
            "export enum InviteRole { Admin, Member }\nexport type Invite = { role: InviteRole };\n\
             export enum Level { Low, High }\n\
             export namespace Paths { export const home = '/'; export const admin = '/admin'; }\n\
             export function createStore(...args: unknown[]) { return args; }\n"
                .to_owned(),
        ),
        (
            "src/routes.ts",
            "export const list = '/list';\nexport const detail = '/detail';\n\
             export type Route = string;\nexport type Params = { id: string };\n"
                .to_owned(),
        ),
        (
            "src/role-selector.ts",
            "import { InviteRole, Level, Paths, type Invite } from './store';\nimport * as routes from './routes';\n\
             export const pick = (invite: Invite, params: routes.Params) =>\n\
             [invite.role === InviteRole.Admin, Level.High, Paths.admin, routes.detail, params];\n"
                .to_owned(),
        ),
    ]
}

#[test]
fn exports_and_types_are_not_judged_in_a_package_with_incomplete_entries() {
    const RULES: [&str; 7] = [
        "unused-files",
        "unused-exports",
        "unused-types",
        "unused-enum-members",
        "unused-namespace-members",
        "unused-namespace-exports",
        "unused-namespace-types",
    ];
    // With entries that are fully known, every rule reports what only `role-selector.ts` uses: the control that makes
    // the assertions below mean something.
    let diagnostics = lint_owned(&incomplete_entries_workspace("./src/main.ts"));
    let expected = [
        ("unused-files", "src/role-selector.ts: src/role-selector.ts"),
        ("unused-exports", "src/store.ts: InviteRole"),
        ("unused-types", "src/store.ts: Invite"),
        ("unused-enum-members", "src/store.ts: Level.High"),
        ("unused-namespace-members", "src/store.ts: Paths.admin"),
        ("unused-namespace-exports", "src/routes.ts: detail"),
        ("unused-namespace-types", "src/routes.ts: Params"),
    ];
    for (rule, finding) in expected {
        assert_eq!(found(&diagnostics, rule), [finding], "{rule}: {diagnostics:#?}");
    }
    assert_eq!(expected.map(|(rule, _)| rule), RULES);

    // `dist/server.js` maps to no source, so the package's real entries are unknown and `role-selector.ts` may be one.
    let diagnostics = lint_owned(&incomplete_entries_workspace("./dist/server.js"));
    for rule in RULES {
        assert_eq!(found(&diagnostics, rule), none(), "{rule}: {diagnostics:#?}");
    }
}

#[test]
fn optional_dependencies_are_not_reported_as_unused() {
    // Platform packages are loaded by a computed name (`import(`@lydell/node-pty-${platform}`)`), as in knip, which
    // never reports `optionalDependencies`.
    let package = manifest(&format!(
        r#""name":"app","version":"1.0.0","private":true,{PNPM},"main":"index.js","dependencies":{{"left-pad":"1.3.0"}},"optionalDependencies":{{"@lydell/node-pty-linux-x64":"1.1.0","@lydell/node-pty-darwin-arm64":"1.1.0"}}"#
    ));
    let files = [
        ("package.json", package.as_str()),
        (
            "index.js",
            "const platform = `${process.platform}-${process.arch}`;\nexport default () => import(`@lydell/node-pty-${platform}`);\n",
        ),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        vec!["package.json: left-pad".to_owned()],
        "{diagnostics:#?}"
    );
}

#[test]
fn commands_after_cd_into_a_workspace_package_are_judged_there() {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"scripts":{{"serve":"cd sub && pnpm exec rspack serve","start":"cd sub && node -r @x/register-hook ./start.js","docs":"cd docs && rspack build"}}"#
    ));
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - sub\n"),
        (
            "sub/package.json",
            r#"{"name":"sub","private":true,"devDependencies":{"@rspack/cli":"1.0.0","@x/register-hook":"1.0.0"}}"#,
        ),
        ("sub/start.js", "console.log('start');\n"),
    ];
    let diagnostics = lint_files(&files, "");
    // `cd sub` runs `rspack` and loads the hook with what `sub` lists; `docs` is no workspace package, so `rspack`
    // there is still a binary of the root.
    assert_eq!(
        found(&diagnostics, "unlisted-binaries"),
        ["package.json: rspack"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        none(),
        "{diagnostics:#?}"
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), none(), "{diagnostics:#?}");
    assert_eq!(found(&diagnostics, "unused-files"), none(), "{diagnostics:#?}");
}

/// A member `tool` with the binary `mytool`, which `app` runs without listing `tool`, `web` runs listing it and `tool`
/// runs in a script of its own; the unlisted binaries found with `files` added, sorted.
fn unlisted_member_binaries(files: &[(&str, &str)]) -> Vec<String> {
    let mut all = vec![
        (
            "packages/tool/package.json",
            r#"{"name":"tool","version":"1.0.0","bin":{"mytool":"./bin/mytool.js"},"scripts":{"usage":"mytool --help"}}"#,
        ),
        ("packages/tool/bin/mytool.js", "#!/usr/bin/env node\n"),
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.js","scripts":{"build":"mytool build"}}"#,
        ),
        ("packages/app/src/index.js", "module.exports = 1;\n"),
        (
            "packages/web/package.json",
            r#"{"name":"web","version":"1.0.0","main":"src/index.js","scripts":{"build":"mytool build"},"devDependencies":{"tool":"1.0.0"}}"#,
        ),
        ("packages/web/src/index.js", "module.exports = 2;\n"),
    ];
    all.extend_from_slice(files);
    let mut unlisted = found(&lint_files(&all, ""), "unlisted-binaries");
    unlisted.sort();
    unlisted
}

/// pnpm and Yarn Berry put a member's binary on the `PATH` only of the packages that depend on it, so `app` runs an
/// unlisted binary; npm, Yarn Classic and Bun link every member's binaries into the root `node_modules/.bin`, so it
/// does not. A package that lists the member, and the member itself, run it under every package manager.
#[test]
fn a_member_binary_is_provided_where_the_package_manager_links_it() {
    let pnpm = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0"}"#;
    let isolated = unlisted_member_binaries(&[
        ("package.json", pnpm),
        ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
    ]);
    assert_eq!(isolated, ["packages/app/package.json: mytool"]);
    let berry = r#"{"name":"root","private":true,"packageManager":"yarn@4.5.0","workspaces":["packages/*"]}"#;
    assert_eq!(
        unlisted_member_binaries(&[("package.json", berry)]),
        ["packages/app/package.json: mytool"]
    );
    for manager in ["npm@10.0.0", "yarn@1.22.22", "bun@1.3.0"] {
        let root =
            format!(r#"{{"name":"root","private":true,"packageManager":"{manager}","workspaces":["packages/*"]}}"#);
        assert_eq!(
            unlisted_member_binaries(&[("package.json", &root)]),
            none(),
            "{manager}"
        );
    }
}

/// In a Rush repository, `rush`, `rushx` and `rush-pnpm` are Rush's own commands, and a member's binary is linked only
/// for the packages that depend on it, as with pnpm.
#[test]
fn rush_commands_are_provided_in_a_rush_repository() {
    let rush = r#"{"rushVersion":"5.150.0","pnpmVersion":"9.0.0","projects":[{"packageName":"tool","projectFolder":"packages/tool"},{"packageName":"app","projectFolder":"packages/app"},{"packageName":"web","projectFolder":"packages/web"},{"packageName":"ops","projectFolder":"packages/ops"}]}"#;
    let ops = r#"{"name":"ops","version":"1.0.0","main":"index.js","scripts":{"ci":"rush build && rushx test && rush-pnpm install"}}"#;
    let files = [
        ("rush.json", rush),
        ("packages/ops/package.json", ops),
        ("packages/ops/index.js", "module.exports = 3;\n"),
    ];
    assert_eq!(unlisted_member_binaries(&files), ["packages/app/package.json: mytool"]);
    // Without `rush.json` they are binaries like any other.
    let pnpm = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0"}"#;
    let elsewhere = unlisted_member_binaries(&[
        ("package.json", pnpm),
        ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
        ("packages/ops/package.json", ops),
        ("packages/ops/index.js", "module.exports = 3;\n"),
    ]);
    assert_eq!(
        elsewhere,
        [
            "packages/app/package.json: mytool",
            "packages/ops/package.json: rush",
            "packages/ops/package.json: rush-pnpm",
            "packages/ops/package.json: rushx"
        ]
    );
}

/// A workflow step and a source file that run a command after `cd` into a workspace package run it there: a provider
/// that package lists is used and nothing is unlisted at the file. A directory in no other package leaves the command
/// at the file, as before.
#[test]
fn workflow_and_source_commands_after_cd_into_a_workspace_package_are_judged_there() {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"main":"scripts/build.mjs""#
    ));
    let workflow = "name: css\non: [push]\njobs:\n  scan:\n    runs-on: ubuntu-latest\n    steps:\n      \
                    - run: cd sub && pnpm exec rspack build\n      - run: cd docs && pnpm exec vitepress build\n";
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - sub\n"),
        (
            "sub/package.json",
            r#"{"name":"sub","private":true,"main":"index.js","devDependencies":{"@rspack/cli":"1.0.0"}}"#,
        ),
        ("sub/index.js", "module.exports = 1;\n"),
        (".github/workflows/css.yml", workflow),
        (
            "scripts/build.mjs",
            "import { execSync } from 'node:child_process';\n\
             execSync('cd sub && rspack serve');\nexecSync('cd tools && tsc -b');\n",
        ),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(
        found(&diagnostics, "unlisted-binaries"),
        [".github/workflows/css.yml: vitepress", "scripts/build.mjs: tsc"],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        none(),
        "{diagnostics:#?}"
    );
}

/// A source file that runs a command with a `cwd` its own location computes runs it there: in another workspace
/// package, a provider that package lists is used and nothing is unlisted at the file.
#[test]
fn source_commands_with_a_cwd_of_the_file_run_in_that_directory() {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"main":"scripts/build.mjs""#
    ));
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - sub\n"),
        (
            "sub/package.json",
            r#"{"name":"sub","private":true,"main":"index.js","devDependencies":{"@rspack/cli":"1.0.0"}}"#,
        ),
        ("sub/index.js", "module.exports = 1;\n"),
        (
            "scripts/build.mjs",
            "import { execSync } from 'node:child_process';\nimport path from 'node:path';\n\
             execSync('rspack build', { cwd: path.join(import.meta.dirname, '../sub') });\n",
        ),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(found(&diagnostics, "unlisted-binaries"), none(), "{diagnostics:#?}");
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        none(),
        "{diagnostics:#?}"
    );
}

/// A binary that a package manager runs in another workspace package's directory without naming a workspace
/// (`pnpm -C sub exec`) is credited to that package wherever the command is read, a workflow as a script; one whose
/// command names the workspaces it runs in (`--filter sub`) is not credited to the directory it starts in.
#[test]
fn filtered_binaries_are_credited_to_their_directory_only_without_named_workspaces() {
    let root = manifest(&format!(
        r#""name":"root","private":true,{PNPM},"main":"scripts/build.mjs""#
    ));
    let workflow = "name: build\non: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      \
                    - run: pnpm -C sub exec rspack build\n";
    let files = [
        ("package.json", root.as_str()),
        ("pnpm-workspace.yaml", "packages:\n  - sub\n  - web\n"),
        (
            "sub/package.json",
            r#"{"name":"sub","private":true,"main":"index.js","devDependencies":{"@rspack/cli":"1.0.0"}}"#,
        ),
        ("sub/index.js", "module.exports = 1;\n"),
        (
            "web/package.json",
            r#"{"name":"web","private":true,"main":"index.js","devDependencies":{"rimraf":"6.0.0"}}"#,
        ),
        ("web/index.js", "module.exports = 2;\n"),
        (".github/workflows/build.yml", workflow),
        (
            "scripts/build.mjs",
            "import { execSync } from 'node:child_process';\n\
             execSync('cd web && pnpm --filter sub exec rimraf dist');\n",
        ),
    ];
    let diagnostics = lint_files(&files, "");
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["web/package.json: rimraf"],
        "{diagnostics:#?}"
    );
    assert_eq!(found(&diagnostics, "unlisted-binaries"), none(), "{diagnostics:#?}");
}
