//! Packages that tool configs name (ADR 0086): a `tsconfig` `extends`, `types` or plugin, an oxlint or eslint plugin
//! or shareable config, a Prisma generator, a Babel preset or plugin and a postcss plugin are unlisted dependencies at
//! the config when no manifest lists them, and uses of the manifest that does.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

fn lint_with(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
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

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    lint_with(files, "")
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

fn none() -> Vec<String> {
    Vec::new()
}

/// A `tsconfig` names packages in `extends`, `compilerOptions.types` and `compilerOptions.plugins`; a relative
/// `extends` is a further config, not a package.
#[test]
fn packages_a_tsconfig_names_are_unlisted_at_the_tsconfig() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"typescript":"5.0.0","@tsconfig/node20":"1.0.0"}}"#,
        ),
        (
            "tsconfig.json",
            r#"{"extends":["@tsconfig/node20/tsconfig.json","@acme/tsconfig/base.json","./tsconfig.paths.json"],
               "compilerOptions":{"types":["vitest/globals"],"plugins":[{"name":"@effect/language-service"}]}}"#,
        ),
        ("tsconfig.paths.json", r#"{"compilerOptions":{"strict":true}}"#),
        ("src/index.ts", "export const a = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        [
            "tsconfig.json: @acme/tsconfig",
            "tsconfig.json: @effect/language-service",
            "tsconfig.json: vitest",
        ]
    );
    let finding = diagnostics
        .iter()
        .find(|d| d.dependency.as_deref() == Some("@acme/tsconfig"))
        .unwrap();
    assert_eq!(
        finding.message,
        "`tsconfig.json` references `@acme/tsconfig`, which is not listed in `package.json`"
    );
    assert_eq!(finding.detail.as_deref(), Some("ConfigReference"));
    assert!(
        !found(&diagnostics, "unused-dev-dependencies").contains(&"package.json: @tsconfig/node20".to_owned()),
        "a listed package the tsconfig extends is used"
    );
}

/// Oxlint `jsPlugins` and eslint plugins and shareable configs name packages; a built-in config (`eslint:recommended`),
/// a local plugin file and an import resolver (an optional reference) do not need one listed.
#[test]
fn lint_plugins_and_shareable_configs_are_unlisted_at_the_lint_config() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"oxlint":"1.0.0","eslint":"8.0.0","eslint-plugin-react":"7.0.0"}}"#,
        ),
        (
            ".oxlintrc.json",
            r#"{"jsPlugins":["eslint-plugin-lodash","./rules/local.js"]}"#,
        ),
        ("rules/local.js", "export default { rules: {} };\n"),
        (
            ".eslintrc.json",
            r#"{"extends":["eslint:recommended","plugin:react/recommended","airbnb"],"plugins":["import"],
               "settings":{"import/resolver":{"typescript":{}}}}"#,
        ),
        ("src/index.ts", "export const a = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        [
            ".eslintrc.json: eslint-config-airbnb",
            ".eslintrc.json: eslint-plugin-import",
            ".oxlintrc.json: eslint-plugin-lodash",
        ]
    );
}

/// A Prisma generator's `provider`, Babel presets and plugins and postcss plugins name packages; a built-in generator,
/// a relative provider or plugin and a postcss plugin turned off do not.
#[test]
fn generators_presets_and_postcss_plugins_are_unlisted_at_their_file() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"prisma":"6.0.0","@babel/core":"7.0.0","postcss":"8.0.0"}}"#,
        ),
        (
            "prisma/schema.prisma",
            "generator client {\n  provider = \"prisma-client-js\"\n}\n\ngenerator kysely {\n  provider = \"prisma-kysely\"\n}\n\ngenerator local {\n  provider = \"./gen.js\"\n}\n",
        ),
        ("prisma/gen.js", "module.exports = {};\n"),
        (
            "babel.config.json",
            r#"{"presets":["@babel/preset-env"],"plugins":["./babel/local-plugin.js","module-resolver"]}"#,
        ),
        ("babel/local-plugin.js", "module.exports = () => ({});\n"),
        (
            "postcss.config.json",
            r#"{"plugins":{"autoprefixer":{},"postcss-nesting":false,"./postcss/local.js":{}}}"#,
        ),
        ("postcss/local.js", "module.exports = () => ({});\n"),
        ("src/index.ts", "export const a = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        [
            "babel.config.json: @babel/preset-env",
            "babel.config.json: babel-plugin-module-resolver",
            "postcss.config.json: autoprefixer",
            "prisma/schema.prisma: prisma-kysely",
        ]
    );
}

/// A package listed by the config's package or a package above it, a private package's workspace member, a package a
/// command of a config names and an optional reference are not unlisted; a config is judged against the package whose
/// directory holds it.
#[test]
fn listed_member_command_and_optional_references_are_not_unlisted() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"typescript":"5.0.0","@tsconfig/strictest":"2.0.0","lint-staged":"15.0.0","vite":"6.0.0"}}"#,
        ),
        (
            ".lintstagedrc.json",
            r#"{"*.ts":"node --require=esbuild-register check.ts"}"#,
        ),
        ("check.ts", "export {};\n"),
        (
            "vite.config.ts",
            "export default { optimizeDeps: { include: ['@acme/prebundled'] } };\n",
        ),
        (
            "packages/tsconfig/package.json",
            r#"{"name":"@acme/tsconfig","version":"1.0.0","private":true,"files":["base.json"]}"#,
        ),
        ("packages/tsconfig/base.json", r#"{"compilerOptions":{"strict":true}}"#),
        (
            "packages/app/package.json",
            r#"{"name":"@acme/app","version":"1.0.0","private":true,"main":"src/index.ts"}"#,
        ),
        (
            "packages/app/tsconfig.json",
            r#"{"extends":["@acme/tsconfig/base.json","@tsconfig/strictest/tsconfig.json"]}"#,
        ),
        ("packages/app/src/index.ts", "export const a = 1;\n"),
        (
            "packages/lib/package.json",
            r#"{"name":"@acme/lib","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        (
            "packages/lib/tsconfig.json",
            r#"{"extends":"@acme/tsconfig/base.json"}"#,
        ),
        ("packages/lib/src/index.ts", "export const b = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["packages/lib/tsconfig.json: @acme/tsconfig"]
    );
    assert!(
        !found(&diagnostics, "unused-dev-dependencies").contains(&"package.json: @tsconfig/strictest".to_owned()),
        "a package listed above the config is used"
    );
}

/// `ignore-dependencies` silences a package, `ignore` a config file, a config in a fixture directory is not judged,
/// and production mode judges no config.
#[test]
fn ignores_fixtures_and_production_mode_apply_to_config_references() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"typescript":"5.0.0"}}"#,
        ),
        (
            "tsconfig.json",
            r#"{"extends":["./config/tsconfig.base.json","./fixtures/tsconfig.json","@acme/silenced/base.json","@acme/reported/base.json"]}"#,
        ),
        ("config/tsconfig.base.json", r#"{"extends":"@acme/hidden/base.json"}"#),
        ("fixtures/tsconfig.json", r#"{"extends":"@acme/fixture/base.json"}"#),
        ("src/index.ts", "export const a = 1;\n"),
    ];
    let toml = "[js]\nignore = [\"config/**\"]\nignore-dependencies = [\"@acme/silenced\"]\n";
    assert_eq!(
        found(&lint_with(&files, toml), "unlisted-dependencies"),
        ["tsconfig.json: @acme/reported"]
    );
    assert_eq!(
        found(&lint_with(&files, ""), "unlisted-dependencies"),
        [
            "config/tsconfig.base.json: @acme/hidden",
            "tsconfig.json: @acme/reported",
            "tsconfig.json: @acme/silenced",
        ]
    );
    let production = "[js]\nproduction = true\n";
    assert_eq!(found(&lint_with(&files, production), "unlisted-dependencies"), none());
}

/// `compilerOptions.types` names the package whose types it loads: a listing of the package or of its `@types`
/// package by the config's package or a package above it satisfies it, and a name that is a `@types` package or a
/// scoped package is that package (knip's `isTypeOnly`).
#[test]
fn tsconfig_types_are_satisfied_by_the_package_or_its_types_package() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"typescript":"5.0.0","cypress":"13.0.0","@types/node":"22.0.0","@types/jest":"29.0.0"}}"#,
        ),
        (
            "packages/app/package.json",
            r#"{"name":"@acme/app","version":"1.0.0","main":"src/index.ts","devDependencies":{"@cloudflare/workers-types":"4.0.0"}}"#,
        ),
        (
            "packages/app/tsconfig.json",
            r#"{"compilerOptions":{"types":["cypress","@cloudflare/workers-types","@types/node","jest","mocha"]}}"#,
        ),
        ("packages/app/src/index.ts", "export const a = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["packages/app/tsconfig.json: mocha"]
    );
    let unused = found(&diagnostics, "unused-dev-dependencies");
    for listed in [
        "package.json: @types/jest",
        "packages/app/package.json: @cloudflare/workers-types",
    ] {
        assert!(!unused.contains(&listed.to_owned()), "{listed} is used: {unused:?}");
    }
}

/// A compiler option of a shared base config (`types`, `jsxImportSource`, `importHelpers`, `plugins`) applies to each
/// package extending it, which may list the package in place of the base config's owner (knip's
/// `alsoReferenceOwner`); a package that does not is reported at the base config, unless its `ignore-dependencies`
/// names the package.
#[test]
fn compiler_options_of_a_shared_base_config_are_satisfied_by_the_extending_package() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"typescript":"5.0.0"}}"#,
        ),
        (
            "packages/tsconfig/package.json",
            r#"{"name":"@acme/tsconfig","version":"1.0.0","private":true,"files":["base.json"]}"#,
        ),
        (
            "packages/tsconfig/base.json",
            r#"{"compilerOptions":{"types":["vitest/globals"],"jsxImportSource":"preact","importHelpers":true,"plugins":[{"name":"@effect/language-service"}]}}"#,
        ),
        (
            "packages/app/package.json",
            r#"{"name":"@acme/app","version":"1.0.0","main":"src/index.ts","devDependencies":{"vitest":"3.0.0","preact":"10.0.0","tslib":"2.0.0","@effect/language-service":"0.1.0"}}"#,
        ),
        ("packages/app/tsconfig.json", r#"{"extends":"../tsconfig/base.json"}"#),
        ("packages/app/src/index.ts", "export const a = 1;\n"),
        (
            "packages/lib/package.json",
            r#"{"name":"@acme/lib","version":"1.0.0","main":"src/index.ts","devDependencies":{"vitest":"3.0.0","preact":"10.0.0","@effect/language-service":"0.1.0"}}"#,
        ),
        ("packages/lib/tsconfig.json", r#"{"extends":"../tsconfig/base.json"}"#),
        ("packages/lib/src/index.ts", "export const b = 1;\n"),
    ];
    assert_eq!(
        found(&lint(&files), "unlisted-dependencies"),
        ["packages/tsconfig/base.json: tslib"]
    );
    let toml = "[js.workspaces.\"packages/lib\"]\nignore-dependencies = [\"tslib\"]\n";
    assert_eq!(found(&lint_with(&files, toml), "unlisted-dependencies"), none());
}

/// A config that git ignores is generated (`.nuxt/tsconfig.json`) and not judged; `[js] include-fixtures` judges a
/// config in a fixture directory like any other.
#[test]
fn gitignored_configs_are_not_judged_and_include_fixtures_judges_fixture_configs() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"typescript":"5.0.0"}}"#,
        ),
        (".gitignore", ".nuxt\n"),
        (
            "tsconfig.json",
            r#"{"extends":["./.nuxt/tsconfig.json","./fixtures/tsconfig.json"]}"#,
        ),
        (".nuxt/tsconfig.json", r#"{"extends":"@acme/generated/base.json"}"#),
        ("fixtures/tsconfig.json", r#"{"extends":"@acme/fixture/base.json"}"#),
        ("src/index.ts", "export const a = 1;\n"),
    ];
    assert_eq!(found(&lint(&files), "unlisted-dependencies"), none());
    assert_eq!(
        found(
            &lint_with(&files, "[js]\ninclude-fixtures = true\n"),
            "unlisted-dependencies"
        ),
        ["fixtures/tsconfig.json: @acme/fixture"]
    );
}

/// A config that imports the package it names is reported once, for the import.
#[test]
fn a_config_that_imports_the_package_it_names_is_reported_once() {
    let diagnostics = lint(&[
        (
            "package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","devDependencies":{"prettier":"3.0.0"}}"#,
        ),
        (
            "prettier.config.mjs",
            "import 'prettier-plugin-tailwindcss';\nexport default { plugins: ['prettier-plugin-tailwindcss'] };\n",
        ),
        ("src/index.ts", "export const a = 1;\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["prettier.config.mjs: prettier-plugin-tailwindcss"]
    );
}
