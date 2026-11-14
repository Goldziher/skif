//! Uses of listed dependencies that only a reference outside an ordinary import makes: a re-export inside a
//! `declare module` block, a package file a stylesheet `url()` names and a file of a subpath directory with a
//! `package.json` of its own, and the `@types` package that a package shipping its own types does not use.
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

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("").unwrap();
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

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display(),
                d.dependency.clone().unwrap_or_else(|| d.message.clone())
            )
        })
        .collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

#[test]
fn reexports_inside_declare_module_blocks_use_the_packages_they_name() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"@commander-js/extra-typings":"12.0.0","@types/react-is":"19.0.0","left-pad":"1.3.0"}}"#,
        ),
        ("src/index.ts", "export const main = (): number => 1;\n"),
        (
            "src/commander.d.ts",
            "declare module 'commander' {\n  export * from '@commander-js/extra-typings';\n}\n",
        ),
        (
            "src/react-is.d.ts",
            "declare module 'compiled/react-is' {\n  export { isElement } from 'react-is';\n}\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["package.json: left-pad"]
    );
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
}

#[test]
fn stylesheet_urls_of_package_files_use_the_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","dependencies":{"@acme/fonts":"1.0.0","legacy-icons":"1.0.0","left-pad":"1.3.0"}}"#,
        ),
        ("src/index.ts", "import './index.css';\nexport const main = 1;\n"),
        (
            "src/index.css",
            "@font-face {\n  src: url(\"@acme/fonts/mono/regular.woff2\") format(\"woff2\");\n}\n\
             .icon { background: url(~legacy-icons/sprite.svg); }\n\
             .logo { background: url(\"./logo.png\"), url(images/bg.png), url(\"@/assets/hero.png\"); }\n\
             .remote { background: url(https://cdn.test/a.png); }\n",
        ),
        ("src/logo.png", ""),
        ("src/images/bg.png", ""),
    ]);
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: left-pad"]);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
}

#[test]
fn stylesheet_urls_through_aliases_are_no_unlisted_imports() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","dependencies":{"@fontsource/inter":"5.0.0","left-pad":"1.3.0"}}"#,
        ),
        (
            "node_modules/@acme/icons/package.json",
            r#"{"name":"@acme/icons","version":"1.0.0"}"#,
        ),
        ("src/index.ts", "import './index.css';\nexport const main = 1;\n"),
        (
            "src/index.css",
            "@font-face {\n  src: url('@fontsource/inter/files/inter-latin-400.woff2') format('woff2');\n}\n\
             .hero { background: url('@assets/images/bg.png'); }\n\
             .logo { background: url(~assets/logo.svg), url(\"~static/icons/home.svg\"); }\n\
             .icon { background: url(~@acme/icons/home.svg); }\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: left-pad"]);
    // `@acme/icons` is installed, so its `url()` needs it listed; the aliases name no package skif can find.
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["src/index.css: @acme/icons"]
    );
}

#[test]
fn a_published_subpath_directory_with_a_manifest_belongs_to_its_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"lib","version":"1.0.0","exports":{".":"./index.js","./server":{"import":"./server/index.mjs","require":"./server/index.js"}},"files":["index.js","server"],"dependencies":{"server-impl":"1.0.0","left-pad":"1.3.0"}}"#,
        ),
        ("index.js", "module.exports = 1;\n"),
        ("server/package.json", r#"{"main":"index.js","module":"index.mjs"}"#),
        ("server/index.js", "module.exports = require('server-impl');\n"),
        ("server/index.mjs", "export * from 'server-impl';\n"),
        ("cases/package.json", r#"{"main":"index.js"}"#),
        ("cases/index.js", "module.exports = require('case-only');\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: left-pad"]);
    // `cases` is no subpath `lib` publishes, so its manifest still makes a project of its own.
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn a_package_that_ships_its_own_types_does_not_use_its_types_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"eslint":"9.0.0","@types/eslint":"9.0.0","react-is":"19.0.0","@types/react-is":"19.0.0"}}"#,
        ),
        (
            "node_modules/eslint/package.json",
            r#"{"name":"eslint","version":"9.0.0","types":"lib/types/index.d.ts"}"#,
        ),
        (
            "node_modules/eslint/lib/types/index.d.ts",
            "export namespace Linter {}\n",
        ),
        (
            "node_modules/react-is/package.json",
            r#"{"name":"react-is","version":"19.0.0","main":"index.js"}"#,
        ),
        (
            "src/index.ts",
            "import type { Linter } from 'eslint';\nimport { isElement } from 'react-is';\n\
             export const check = (config: Linter.Config): boolean => isElement(config);\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["package.json: @types/eslint"]
    );
}

#[test]
fn only_a_types_file_the_installed_package_holds_replaces_its_types_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"typed":"1.0.0","@types/typed":"1.0.0","blank":"1.0.0","@types/blank":"1.0.0","nulled":"1.0.0","@types/nulled":"1.0.0","missing":"1.0.0","@types/missing":"1.0.0"}}"#,
        ),
        (
            "node_modules/typed/package.json",
            r#"{"name":"typed","version":"1.0.0","types":"index"}"#,
        ),
        ("node_modules/typed/index.d.ts", "export declare const a: number;\n"),
        (
            "node_modules/blank/package.json",
            r#"{"name":"blank","version":"1.0.0","types":""}"#,
        ),
        (
            "node_modules/nulled/package.json",
            r#"{"name":"nulled","version":"1.0.0","types":null}"#,
        ),
        (
            "node_modules/missing/package.json",
            r#"{"name":"missing","version":"1.0.0","types":"dist/index.d.ts"}"#,
        ),
        (
            "src/index.ts",
            "import { a } from 'typed';\nimport { b } from 'blank';\nimport { c } from 'nulled';\n\
             import { d } from 'missing';\nexport const sum = a + b + c + d;\n",
        ),
    ]);
    // An empty or null `types` declares nothing, and a `types` file the package lacks is not read: those packages
    // keep their `@types` package used.
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["package.json: @types/typed"]
    );
}

#[test]
fn a_subpath_import_of_a_typed_package_uses_its_types_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","devDependencies":{"kit":"1.0.0","@types/kit":"1.0.0"}}"#,
        ),
        (
            "node_modules/kit/package.json",
            r#"{"name":"kit","version":"1.0.0","types":"index.d.ts"}"#,
        ),
        ("node_modules/kit/index.d.ts", "export declare const root: number;\n"),
        (
            "src/index.ts",
            "import { root } from 'kit';\nimport { helper } from 'kit/helpers';\n\
             export const value = helper(root);\n",
        ),
    ]);
    // The types of `kit` cover `kit` itself, not `kit/helpers`, which `@types/kit/helpers.d.ts` may declare.
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
}

#[test]
fn the_main_of_a_subpath_directory_published_through_files_is_an_entry() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"lib","version":"1.0.0","main":"index.js","files":["index.js","server","client/**"],"dependencies":{"server-impl":"1.0.0","client-impl":"1.0.0","left-pad":"1.3.0"}}"#,
        ),
        ("index.js", "module.exports = 1;\n"),
        ("server/package.json", r#"{"main":"index.js"}"#),
        ("server/index.js", "module.exports = require('./impl');\n"),
        ("server/impl.js", "module.exports = require('server-impl');\n"),
        ("client/package.json", r#"{"browser":"./entry.js"}"#),
        ("client/entry.js", "module.exports = require('client-impl');\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-files"), none());
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json: left-pad"]);
    assert_eq!(found(&diagnostics, "unlisted-dependencies"), none());
}
