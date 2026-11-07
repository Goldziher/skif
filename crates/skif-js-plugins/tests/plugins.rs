//! One fixture per plugin: enablement, entries and config-referenced dependencies, plus table and
//! robustness checks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::Path;

use indexmap::IndexMap;
use skif_config::{PluginConfig, PluginOptions};
use skif_core::MemoryFs;
use skif_js_plugins::registry;
use skif_js_plugins::{PluginContext, PluginId, PluginOutput, resolve_plugins};

const DIR: &str = "/ws";

const ADR_IDS: [&str; 113] = [
    "typescript",
    "eslint",
    "prettier",
    "vite",
    "vitest",
    "jest",
    "next",
    "storybook",
    "playwright",
    "tailwind",
    "postcss",
    "babel",
    "webpack",
    "rspack",
    "rollup",
    "tsup",
    "turbo",
    "husky",
    "lint-staged",
    "commitlint",
    "changesets",
    "github-actions",
    "mastra",
    "aws-cdk",
    "browser-extension",
    "tsdown",
    "oxlint",
    "oxfmt",
    "angular",
    "karma",
    "esbuild",
    "svelte",
    "nuxt",
    "astro",
    "drizzle",
    "ts-node",
    "tsx",
    "nx",
    "cypress",
    "mocha",
    "sveltekit",
    "prisma",
    "graphql-codegen",
    "docusaurus",
    "vitepress",
    "wrangler",
    "expo",
    "react-native",
    "metro",
    "release-it",
    "semantic-release",
    "lefthook",
    "simple-git-hooks",
    "stylelint",
    "markdownlint",
    "cspell",
    "nodemon",
    "pm2",
    "pnpm",
    "yarn",
    "syncpack",
    "node",
    "expo-router",
    "tanstack-router",
    "ava",
    "tap",
    "bun",
    "biome",
    "c8",
    "dotenv",
    "electron-vite",
    "gatsby",
    "i18next-parser",
    "jasmine",
    "knex",
    "lingui",
    "msw",
    "nest",
    "netlify",
    "nitro",
    "nyc",
    "oclif",
    "panda-css",
    "plop",
    "preconstruct",
    "react-router",
    "remark",
    "remix",
    "rolldown",
    "rsbuild",
    "rslib",
    "sanity",
    "sentry",
    "size-limit",
    "stryker",
    "svgo",
    "svgr",
    "swc",
    "tsd",
    "typedoc",
    "unbuild",
    "unocss",
    "vercel",
    "vike",
    "webdriver-io",
    "pre-commit",
    "parcel",
    "next-intl",
    "dependency-cruiser",
    "commitizen",
    "electron-forge",
    "serverless-framework",
    "github-action",
];

fn fs_with(files: &[(&str, &str)]) -> MemoryFs {
    let mut fs = MemoryFs::new();
    if !files.iter().any(|(path, _)| *path == "package.json") {
        fs.insert(format!("{DIR}/package.json"), r#"{"name":"app"}"#);
    }
    for (path, content) in files {
        fs.insert(format!("{DIR}/{path}"), content);
    }
    fs
}

fn resolve_with(
    files: &[(&str, &str)],
    dependencies: &[&str],
    is_root: bool,
    plugins: &IndexMap<String, PluginConfig>,
) -> Vec<(PluginId, PluginOutput)> {
    resolve_inheriting(files, dependencies, &[], is_root, plugins)
}

fn resolve_inheriting(
    files: &[(&str, &str)],
    dependencies: &[&str],
    inherited: &[&str],
    is_root: bool,
    plugins: &IndexMap<String, PluginConfig>,
) -> Vec<(PluginId, PluginOutput)> {
    let inherited: Vec<String> = inherited.iter().map(|d| (*d).to_string()).collect();
    let fs = fs_with(files);
    let dependencies: Vec<String> = dependencies.iter().map(|d| (*d).to_string()).collect();
    let scripts = IndexMap::new();
    let ctx = PluginContext {
        dir: Path::new(DIR),
        is_root,
        package_name: Some("app"),
        dependencies: &dependencies,
        inherited_dependencies: &inherited,
        scripts: &scripts,
        fs: &fs,
        plugins,
    };
    resolve_plugins(&ctx)
}

fn resolve_with_scripts(
    files: &[(&str, &str)],
    dependencies: &[&str],
    scripts: &[(&str, &str)],
) -> Vec<(PluginId, PluginOutput)> {
    let fs = fs_with(files);
    let dependencies: Vec<String> = dependencies.iter().map(|d| (*d).to_string()).collect();
    let scripts: IndexMap<String, String> = scripts
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
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
    };
    resolve_plugins(&ctx)
}

fn resolve(files: &[(&str, &str)], dependencies: &[&str]) -> Vec<(PluginId, PluginOutput)> {
    resolve_with(files, dependencies, true, &IndexMap::new())
}

fn output<'a>(results: &'a [(PluginId, PluginOutput)], id: &str) -> &'a PluginOutput {
    results.iter().find(|(plugin, _)| *plugin == id).map_or_else(
        || {
            panic!(
                "plugin {id} is not enabled; enabled: {:?}",
                results.iter().map(|(i, _)| i).collect::<Vec<_>>()
            )
        },
        |(_, output)| output,
    )
}

fn deps(output: &PluginOutput) -> BTreeSet<&str> {
    output.referenced_dependencies.iter().map(|d| d.name.as_str()).collect()
}

fn assert_deps(output: &PluginOutput, expected: &[&str]) {
    let actual = deps(output);
    for name in expected {
        assert!(actual.contains(name), "missing dependency {name}; got {actual:?}");
    }
}

fn assert_entries(entries: &[String], expected: &[&str]) {
    for pattern in expected {
        assert!(
            entries.iter().any(|e| e == pattern),
            "missing entry {pattern}; got {entries:?}"
        );
    }
}

fn assert_absent(output: &PluginOutput, names: &[&str]) {
    let actual = deps(output);
    for name in names {
        assert!(!actual.contains(name), "unexpected dependency {name}; got {actual:?}");
    }
}

#[test]
fn every_adr_plugin_is_registered_exactly_once() {
    let ids: Vec<&str> = registry::plugins().iter().map(|p| p.spec().id).collect();
    assert_eq!(ids, ADR_IDS);
    let unique: BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len());
    for plugin in registry::plugins() {
        let spec = plugin.spec();
        assert!(!spec.knip_commit.is_empty(), "{} lacks a knip commit", spec.id);
        assert!(!spec.enablers.is_empty(), "{} has no enabler", spec.id);
        assert!(registry::plugin(spec.id).is_some());
    }
}

#[test]
fn plugins_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn skif_js_plugins::Plugin>();
}

#[test]
fn nothing_is_enabled_without_enablers() {
    let enabled: Vec<PluginId> = resolve(&[], &[]).into_iter().map(|(id, _)| id).collect();
    assert_eq!(enabled, ["node"], "only the runtime plugin needs no enabler");
}

#[test]
fn typescript() {
    let results = resolve(
        &[
            (
                "tsconfig.json",
                r#"{
                  // comments and trailing commas are allowed
                  "extends": ["@tsconfig/node20/tsconfig.json", "./tsconfig.base.json"],
                  "compilerOptions": {
                    "types": ["node", "vitest/globals"],
                    "jsxImportSource": "@emotion/react",
                    "importHelpers": true,
                    "plugins": [{ "name": "typescript-plugin-css-modules" }],
                  },
                  "references": [{ "path": "./packages/a" }],
                }"#,
            ),
            ("tsconfig.base.json", r#"{"compilerOptions":{"types":["jest"]}}"#),
            (
                "packages/a/tsconfig.json",
                r#"{"compilerOptions":{"importHelpers":true}}"#,
            ),
        ],
        &["typescript"],
    );
    let out = output(&results, "typescript");
    assert_deps(
        out,
        &[
            "@tsconfig/node20",
            "@types/node",
            "vitest",
            "@emotion/react",
            "tslib",
            "typescript-plugin-css-modules",
            "@types/jest",
        ],
    );
    assert!(out.config_files.iter().any(|f| f.ends_with("packages/a/tsconfig.json")));
    assert!(out.provided_binaries.contains(&"tsc".to_string()));
}

#[test]
fn eslint_legacy() {
    let results = resolve(
        &[(
            ".eslintrc.json",
            r#"{
              "extends": ["eslint:recommended", "plugin:@typescript-eslint/recommended", "plugin:react/recommended",
                          "airbnb", "@scope/strict", "prettier"],
              "plugins": ["import", "@scope"],
              "parser": "@typescript-eslint/parser",
              "settings": { "import/resolver": { "typescript": {} } },
              "overrides": [{ "files": ["*.ts"], "extends": ["plugin:jest/recommended"] }]
            }"#,
        )],
        &["eslint"],
    );
    let out = output(&results, "eslint");
    assert_deps(
        out,
        &[
            "@typescript-eslint/eslint-plugin",
            "eslint-plugin-react",
            "eslint-config-airbnb",
            "@scope/eslint-config-strict",
            "eslint-config-prettier",
            "eslint-plugin-import",
            "@scope/eslint-plugin",
            "@typescript-eslint/parser",
            "eslint-import-resolver-typescript",
            "eslint-plugin-jest",
        ],
    );
    assert_absent(out, &["eslint-config-eslint:recommended"]);
}

#[test]
fn eslint_flat_config_is_read_without_error() {
    let results = resolve(
        &[(
            "eslint.config.js",
            "import js from '@eslint/js';\nexport default [js.configs.recommended, { rules: {} }];",
        )],
        &["eslint"],
    );
    let out = output(&results, "eslint");
    assert!(out.config_files.iter().any(|f| f.ends_with("eslint.config.js")));
}

#[test]
fn eslint_config_in_package_json() {
    let results = resolve(
        &[(
            "package.json",
            r#"{"name":"app","eslintConfig":{"extends":["next/core-web-vitals"]}}"#,
        )],
        &["eslint"],
    );
    assert_deps(output(&results, "eslint"), &["eslint-config-next"]);
}

#[test]
fn prettier() {
    let results = resolve(
        &[(
            ".prettierrc",
            r#"{"plugins":["prettier-plugin-tailwindcss"],"overrides":[{"files":"*.md","options":{"plugins":["prettier-plugin-packagejson"]}}]}"#,
        )],
        &["prettier"],
    );
    assert_deps(
        output(&results, "prettier"),
        &["prettier-plugin-tailwindcss", "prettier-plugin-packagejson"],
    );
}

#[test]
fn prettier_shared_config_as_string() {
    let results = resolve(
        &[(
            "package.json",
            r#"{"name":"app","prettier":"@company/prettier-config"}"#,
        )],
        &["prettier"],
    );
    assert_deps(output(&results, "prettier"), &["@company/prettier-config"]);
}

#[test]
fn vite() {
    let results = resolve(
        &[
            (
                "vite.config.ts",
                r#"export default defineConfig({
                  build: { rollupOptions: { input: { main: "src/main.ts" } } },
                  optimizeDeps: { include: ["react > scheduler", "lodash"] },
                });"#,
            ),
            (
                "index.html",
                r#"<html><script type="module" src="/src/index.tsx"></script></html>"#,
            ),
        ],
        &["vite"],
    );
    let out = output(&results, "vite");
    assert_entries(&out.production_entries, &["src/main.ts", "src/index.tsx"]);
    assert_deps(out, &["react", "lodash"]);
}

#[test]
fn vitest() {
    let results = resolve(
        &[(
            "vitest.config.ts",
            r#"export default defineConfig({
              test: {
                environment: "jsdom",
                reporters: ["default", "vitest-sonar-reporter"],
                coverage: { provider: "istanbul" },
                setupFiles: ["./test/setup.ts", "vitest-canvas-mock"],
                include: ["tests/**/*.test.ts"],
              },
            });"#,
        )],
        &["vitest"],
    );
    let out = output(&results, "vitest");
    assert_deps(
        out,
        &[
            "jsdom",
            "vitest-sonar-reporter",
            "@vitest/coverage-istanbul",
            "vitest-canvas-mock",
        ],
    );
    assert_entries(&out.entries, &["test/setup.ts", "tests/**/*.test.ts"]);
    assert!(out.entries.iter().any(|e| e.starts_with("**/*.{bench,test")));
    assert_absent(out, &["default"]);
}

#[test]
fn jest() {
    let results = resolve(
        &[(
            "jest.config.js",
            r#"module.exports = {
              preset: "ts-jest",
              testEnvironment: "jsdom",
              runner: "eslint",
              setupFilesAfterEnv: ["<rootDir>/jest.setup.ts"],
              reporters: ["default", "jest-junit"],
              transform: { "^.+\\.tsx?$": ["ts-jest", {}], "\\.css$": "jest-css-modules-transform" },
              watchPlugins: ["typeahead/filename"],
              moduleNameMapper: {
                "\\.svg$": "<rootDir>/mocks/svg.js",
                "^@/(.*)$": "<rootDir>/src/$1",
                "\\.css$": "identity-obj-proxy",
              },
            };"#,
        )],
        &["jest"],
    );
    let out = output(&results, "jest");
    assert_deps(
        out,
        &[
            "ts-jest",
            "jest-environment-jsdom",
            "jest-runner-eslint",
            "jest-junit",
            "jest-css-modules-transform",
            "jest-watch-typeahead",
            "identity-obj-proxy",
        ],
    );
    assert_entries(&out.entries, &["jest.setup.ts", "mocks/svg.js"]);
    assert_absent(out, &["default"]);
}

#[test]
fn jest_config_in_package_json() {
    let results = resolve(
        &[("package.json", r#"{"name":"app","jest":{"preset":"react-native"}}"#)],
        &["jest"],
    );
    assert_deps(output(&results, "jest"), &["react-native"]);
}

#[test]
fn next() {
    let results = resolve(
        &[(
            "next.config.mjs",
            r#"export default { pageExtensions: ["tsx", "ts", "mdx"] };"#,
        )],
        &["next"],
    );
    let out = output(&results, "next");
    assert_entries(&out.entries, &["next-env.d.ts", "{,src/}pages/**/*.{tsx,ts,mdx}"]);
    assert_entries(&out.production_entries, &["{,src/}pages/**/*.{tsx,ts,mdx}"]);
    assert!(
        out.entries
            .iter()
            .any(|e| e.contains("app/**/{layout,page,route,template}"))
    );
    assert!(out.provided_binaries.contains(&"next".to_string()));
}

#[test]
fn next_without_config_still_has_router_entries() {
    let results = resolve(&[], &["next"]);
    assert!(output(&results, "next").entries.iter().any(|e| e.contains("pages/**")));
}

#[test]
fn storybook() {
    let results = resolve(
        &[(
            ".storybook/main.ts",
            r#"export default {
              stories: ["../src/**/*.stories.@(ts|tsx)", { directory: "../lib", files: "*.mdx" }],
              addons: ["@storybook/addon-essentials", { name: "@storybook/addon-vitest" }],
              framework: { name: "@storybook/react-vite", options: {} },
              core: { builder: "webpack5" },
            };"#,
        )],
        &["@storybook/react-vite"],
    );
    let out = output(&results, "storybook");
    assert_entries(&out.entries, &["src/**/*.stories.{ts,tsx}", "lib/*.mdx"]);
    assert_deps(
        out,
        &[
            "@storybook/addon-essentials",
            "@storybook/addon-vitest",
            "@storybook/react-vite",
            "@storybook/builder-webpack5",
            "@storybook/manager-webpack5",
            "@vitest/coverage-v8",
        ],
    );
    assert!(
        out.referenced_dependencies
            .iter()
            .any(|d| d.name == "@vitest/coverage-v8" && d.optional)
    );
}

#[test]
fn storybook_defaults_to_conventional_stories() {
    let results = resolve(&[(".storybook/main.ts", "export default {};")], &["@storybook/react"]);
    assert!(
        output(&results, "storybook")
            .entries
            .iter()
            .any(|e| e.contains("stories"))
    );
}

#[test]
fn playwright() {
    let results = resolve(
        &[(
            "playwright.config.ts",
            r#"export default defineConfig({
              testDir: "./e2e",
              globalSetup: "./e2e/global-setup.ts",
              reporter: [["html"], ["playwright-allure"]],
              webServer: { command: "vite preview --port 4173" },
              projects: [{ name: "smoke", testMatch: "smoke/*.spec.ts" }],
            });"#,
        )],
        &["@playwright/test"],
    );
    let out = output(&results, "playwright");
    assert_entries(
        &out.entries,
        &[
            "e2e/**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "e2e/smoke/*.spec.ts",
            "e2e/global-setup.ts",
        ],
    );
    assert_deps(out, &["playwright-allure"]);
    assert!(out.referenced_binaries.contains(&"vite".to_string()));
}

#[test]
fn tailwind() {
    let results = resolve(
        &[(
            "tailwind.config.js",
            r#"module.exports = { content: [], plugins: ["@tailwindcss/typography"] };"#,
        )],
        &["tailwindcss"],
    );
    let out = output(&results, "tailwind");
    assert_deps(out, &["@tailwindcss/typography"]);
    assert_entries(&out.entries, &["{,config/}tailwind.config.{js,cjs,mjs,ts,cts,mts}"]);
}

#[test]
fn postcss() {
    let results = resolve(
        &[(
            "postcss.config.json",
            r#"{"plugins":{"tailwindcss":{},"autoprefixer":{},"postcss-nested":false}}"#,
        )],
        &["postcss"],
    );
    let out = output(&results, "postcss");
    assert_deps(out, &["tailwindcss", "autoprefixer", "postcss"]);
    assert_absent(out, &["postcss-nested"]);
}

#[test]
fn postcss_plugins_loaded_with_require_count_as_dependencies() {
    let results = resolve(
        &[(
            "postcss.config.js",
            "module.exports = { plugins: [require('autoprefixer'), require('tailwindcss')({ content: [] }), [require('postcss-nested'), {}]] };",
        )],
        &["postcss"],
    );
    assert_deps(
        output(&results, "postcss"),
        &["autoprefixer", "tailwindcss", "postcss-nested", "postcss"],
    );
}

#[test]
fn babel() {
    let results = resolve(
        &[(
            ".babelrc",
            r#"{
              "presets": ["@babel/env", ["@babel/preset-react", {}], "module:my-preset", "@scope/x"],
              "plugins": ["lodash", "@babel/transform-runtime"],
              "env": { "test": { "plugins": ["istanbul"] } }
            }"#,
        )],
        &["@babel/core"],
    );
    assert_deps(
        output(&results, "babel"),
        &[
            "@babel/preset-env",
            "@babel/preset-react",
            "my-preset",
            "@scope/babel-preset-x",
            "babel-plugin-lodash",
            "@babel/plugin-transform-runtime",
            "babel-plugin-istanbul",
        ],
    );
}

#[test]
fn webpack() {
    let results = resolve(
        &[(
            "webpack.config.js",
            r#"module.exports = {
              entry: { main: "./src/index.js", admin: { import: "./src/admin.js", dependOn: "main" } },
              module: { rules: [
                { test: /\.tsx?$/, use: "ts-loader" },
                { test: /\.css$/, use: ["style-loader", { loader: "css-loader", options: {} }] },
                { test: /\.js$/, loader: "babel-loader", options: { presets: ["@babel/env"] } },
              ] },
            };"#,
        )],
        &["webpack"],
    );
    let out = output(&results, "webpack");
    assert_entries(&out.production_entries, &["src/index.js", "src/admin.js"]);
    assert!(!out.entries.iter().any(|e| e == "main"));
    assert_deps(
        out,
        &[
            "ts-loader",
            "style-loader",
            "css-loader",
            "babel-loader",
            "@babel/preset-env",
        ],
    );
}

#[test]
fn rollup() {
    let results = resolve(
        &[(
            "rollup.config.mjs",
            r#"export default [{ input: "src/a.js" }, { input: { b: "src/b.js" } }];"#,
        )],
        &["rollup"],
    );
    assert_entries(
        &output(&results, "rollup").production_entries,
        &["src/a.js", "src/b.js"],
    );
}

#[test]
fn mastra_entries_and_binary() {
    for dependency in ["mastra", "@mastra/core"] {
        let results = resolve(&[("package.json", r#"{"name":"app"}"#)], &[dependency]);
        let out = output(&results, "mastra");
        assert_entries(
            &out.production_entries,
            &[
                "src/mastra/index.{ts,js,mts,mjs}",
                "src/mastra/workflows/*.{js,ts}",
                "src/mastra/{,agents/*/}tools/**/*.{js,ts}",
            ],
        );
        assert_entries(&out.entries, &["src/mastra/index.{ts,js,mts,mjs}"]);
        assert_eq!(out.provided_binaries, vec!["mastra".to_string()]);
    }
    let results = resolve(&[], &["react"]);
    assert!(!results.iter().any(|(id, _)| *id == "mastra"));
}

#[test]
fn mastra_config_file_is_read() {
    let results = resolve(&[("mastra.config.ts", "export default {};")], &["mastra"]);
    assert_eq!(output(&results, "mastra").config_files.len(), 1);
}

#[test]
fn aws_cdk_app_command_is_a_script() {
    let results = resolve(
        &[(
            "cdk.json",
            r#"{"app":"npx ts-node --prefer-ts-exts app/main.ts","context":{}}"#,
        )],
        &["aws-cdk-lib"],
    );
    let out = output(&results, "aws-cdk");
    assert_entries(&out.entries, &["app/main.ts"]);
    assert!(
        out.referenced_binaries.iter().any(|b| b == "ts-node"),
        "{:?}",
        out.referenced_binaries
    );
    assert_eq!(out.provided_binaries, vec!["cdk".to_string()]);
}

#[test]
fn aws_cdk_enabled_by_the_cli_dependency() {
    let results = resolve(&[("cdk.json", r#"{"app":"tsx bin/app.ts"}"#)], &["aws-cdk"]);
    assert_entries(&output(&results, "aws-cdk").entries, &["bin/app.ts"]);
}

const EXTENSION_MANIFEST: &str = r#"{
  "manifest_version": 3,
  "background": {"service_worker": "dist/background.js"},
  "content_scripts": [{"matches": ["*://*/*"], "js": ["content.js"]}],
  "action": {"default_popup": "popup.html"},
  "options_ui": {"page": "options/options.html"},
  "web_accessible_resources": [{"resources": ["injected.js", "icon.png"], "matches": ["*://*/*"]}]
}"#;

#[test]
fn browser_extension_manifest_entries() {
    let results = resolve(
        &[
            ("manifest.json", EXTENSION_MANIFEST),
            ("popup.html", r#"<script src="popup.js"></script>"#),
            (
                "options/options.html",
                r#"<script type="module" src="./options.js"></script>"#,
            ),
        ],
        &[],
    );
    let out = output(&results, "browser-extension");
    assert_entries(
        &out.entries,
        &[
            "dist/background.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "src/background.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "content.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "injected.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "popup.js",
            "options/options.js",
        ],
    );
    assert!(!out.entries.iter().any(|e| e.contains("icon.png")));
}

#[test]
fn browser_extension_manifest_in_public_and_mv2_shapes() {
    let manifest = r#"{
      "manifest_version": 2,
      "background": {"scripts": ["bg.js"]},
      "browser_action": {"default_popup": "popup.html"},
      "web_accessible_resources": ["inject.js", "style.css"]
    }"#;
    let results = resolve(
        &[
            ("public/manifest.json", manifest),
            ("src/popup.html", r#"<script src="popup.ts"></script>"#),
        ],
        &[],
    );
    let out = output(&results, "browser-extension");
    assert_entries(
        &out.entries,
        &[
            "public/bg.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "src/bg.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "public/inject.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "src/popup.ts",
        ],
    );
    assert!(!out.entries.iter().any(|e| e.contains("style.css")));
}

#[test]
fn a_web_manifest_is_not_a_browser_extension() {
    let results = resolve(&[("manifest.json", r#"{"name":"pwa","start_url":"/"}"#)], &[]);
    assert_eq!(
        output(&results, "browser-extension").entries,
        [] as [std::string::String; 0]
    );
}

#[test]
fn tsup() {
    let results = resolve(
        &[(
            "tsup.config.ts",
            r#"export default defineConfig({ entry: ["src/index.ts", "!src/skip.ts"] });"#,
        )],
        &["tsup"],
    );
    let out = output(&results, "tsup");
    assert_entries(&out.production_entries, &["src/index.ts"]);
    assert!(!out.entries.iter().any(|e| e.contains("skip")));
}

#[test]
fn tsup_config_in_package_json() {
    let results = resolve(
        &[(
            "package.json",
            r#"{"name":"app","tsup":{"entry":{"cli":"src/cli.ts"}}}"#,
        )],
        &["tsup"],
    );
    assert_entries(&output(&results, "tsup").production_entries, &["src/cli.ts"]);
}

#[test]
fn turbo() {
    let results = resolve(&[("turbo.json", r#"{"tasks":{"build":{}}}"#)], &["turbo"]);
    let out = output(&results, "turbo");
    assert!(out.config_files.iter().any(|f| f.ends_with("turbo.json")));
    assert_entries(&out.entries, &["plopfile.{ts,js,cjs,mts,mjs}"]);
    assert!(out.provided_binaries.contains(&"turbo".to_string()));
}

#[test]
fn husky() {
    let hook = "#!/bin/sh\n. \"$(dirname \"$0\")/_/husky.sh\"\nnpx lint-staged\n";
    let files = [(".husky/pre-commit", hook)];
    let results = resolve(&files, &["husky"]);
    let out = output(&results, "husky");
    assert!(
        out.referenced_binaries.contains(&"lint-staged".to_string()),
        "{:?}",
        out.referenced_binaries
    );
    assert!(out.ignore.contains(&".husky/_/**".to_string()));

    let nested = resolve_with(&files, &["husky"], false, &IndexMap::new());
    assert!(nested.iter().all(|(id, _)| *id != "husky"), "husky is root-only");
}

#[test]
fn lint_staged() {
    let results = resolve(
        &[(
            "package.json",
            r#"{"name":"app","lint-staged":{"*.ts":["eslint --fix","prettier --write"],"*.md":"markdownlint"}}"#,
        )],
        &["lint-staged"],
    );
    let out = output(&results, "lint-staged");
    for binary in ["eslint", "prettier", "markdownlint"] {
        assert!(
            out.referenced_binaries.contains(&binary.to_string()),
            "{binary}: {:?}",
            out.referenced_binaries
        );
    }
}

#[test]
fn lint_staged_rc_file() {
    let results = resolve(
        &[(".lintstagedrc.json", r#"{"*.js":"eslint --fix"}"#)],
        &["lint-staged"],
    );
    assert!(
        output(&results, "lint-staged")
            .referenced_binaries
            .contains(&"eslint".to_string())
    );
}

#[test]
fn commitlint() {
    let results = resolve(
        &[(
            "commitlint.config.js",
            r#"module.exports = {
              extends: ["@commitlint/config-conventional", "monorepo", "./local"],
              plugins: ["foo"],
              formatter: "@commitlint/format",
              parserPreset: "conventional-changelog-atom",
            };"#,
        )],
        &["@commitlint/cli"],
    );
    assert_deps(
        output(&results, "commitlint"),
        &[
            "@commitlint/config-conventional",
            "commitlint-config-monorepo",
            "commitlint-plugin-foo",
            "@commitlint/format",
            "conventional-changelog-atom",
        ],
    );
}

#[test]
fn changesets() {
    let results = resolve(
        &[(
            ".changeset/config.json",
            r#"{"changelog":["@changesets/changelog-github",{"repo":"a/b"}],"$schema":"../node_modules/@changesets/config/schema.json"}"#,
        )],
        &["@changesets/cli"],
    );
    assert_deps(
        output(&results, "changesets"),
        &["@changesets/changelog-github", "@changesets/config"],
    );
}

#[test]
fn github_actions() {
    let workflow = "name: ci\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: pnpm exec vitest run\n      - run: |\n          npx playwright test\n      - run: npx other-tool\n        working-directory: packages/x\n";
    let action = "name: local\nruns:\n  using: node20\n  main: dist/index.js\n";
    let results = resolve(
        &[
            (".github/workflows/ci.yml", workflow),
            (".github/actions/local/action.yml", action),
        ],
        &[],
    );
    let out = output(&results, "github-actions");
    assert!(
        out.referenced_binaries.contains(&"vitest".to_string()),
        "{:?}",
        out.referenced_binaries
    );
    assert!(out.referenced_binaries.contains(&"playwright".to_string()));
    assert!(
        !out.referenced_binaries.contains(&"other-tool".to_string()),
        "steps in another directory are skipped"
    );
    assert_entries(&out.entries, &[".github/actions/local/dist/index.js"]);
    assert_eq!(out.config_files.len(), 2);
}

#[test]
fn explicit_switches_override_detection() {
    let files = [("vitest.config.ts", "export default {};")];

    let mut off = IndexMap::new();
    off.insert("vitest".to_string(), PluginConfig::Enabled(false));
    let disabled = resolve_with(&files, &["vitest"], true, &off);
    assert!(disabled.iter().all(|(id, _)| *id != "vitest"));

    let mut on = IndexMap::new();
    on.insert("vitest".to_string(), PluginConfig::Enabled(true));
    let forced = resolve_with(&files, &[], true, &on);
    assert!(forced.iter().any(|(id, _)| *id == "vitest"));
}

#[test]
fn table_overrides_replace_default_globs() {
    let mut plugins = IndexMap::new();
    plugins.insert(
        "playwright".to_string(),
        PluginConfig::Options(
            PluginOptions::new()
                .with_config(["e2e.config.ts"])
                .with_entry(["e2e/**/*.ts"])
                .with_project(["e2e/**"]),
        ),
    );
    let results = resolve_with(
        &[
            ("e2e.config.ts", r#"export default { globalSetup: "./setup.ts" };"#),
            (
                "playwright.config.ts",
                r#"export default { globalSetup: "./other.ts" };"#,
            ),
        ],
        &["@playwright/test"],
        true,
        &plugins,
    );
    let out = output(&results, "playwright");
    assert_eq!(out.project, vec!["e2e/**".to_string()]);
    assert_entries(&out.entries, &["e2e/**/*.ts", "setup.ts"]);
    assert!(!out.entries.iter().any(|e| e == "other.ts"));
    assert!(!out.entries.iter().any(|e| e.starts_with("**/*.{spec")));
}

#[test]
fn table_with_enabled_false_disables() {
    let mut plugins = IndexMap::new();
    plugins.insert(
        "tsup".to_string(),
        PluginConfig::Options(PluginOptions::new().with_enabled(false)),
    );
    let results = resolve_with(&[], &["tsup"], true, &plugins);
    let enabled: Vec<&str> = results.iter().map(|(id, _)| *id).collect();
    assert_eq!(enabled, ["node"], "only the always-on node plugin runs");
}

#[test]
fn malformed_configs_never_panic() {
    let config_files = [
        "tsconfig.json",
        ".eslintrc.json",
        "eslint.config.js",
        ".prettierrc",
        "vite.config.ts",
        "vitest.config.ts",
        "jest.config.js",
        "next.config.js",
        ".storybook/main.ts",
        "playwright.config.ts",
        "tailwind.config.js",
        "postcss.config.json",
        ".babelrc",
        "webpack.config.js",
        "rollup.config.js",
        "tsup.config.ts",
        "turbo.json",
        ".husky/pre-commit",
        ".lintstagedrc",
        "commitlint.config.js",
        ".changeset/config.json",
        ".github/workflows/ci.yml",
        ".github/actions/x/action.yml",
    ];
    let contents = [
        "",
        "{{{",
        "[[[",
        "- - -\n a: [",
        "export default 5;",
        "module.exports = null;",
        "export default [[[], {}, 1, 'x'], null];",
        "\u{0}\u{1}",
        "<script src=",
        r#"{"extends":5,"plugins":{"a":[1,2]},"presets":{"x":1},"entry":[1,{"a":null}],"input":7,"test":"x","stories":3,"steps":[1,{"run":5}],"runs":"x","changelog":9,"$schema":true,"hooks":4,"overrides":"x","references":[1,{"path":2}],"compilerOptions":{"types":1,"plugins":2},"reporters":{"a":1},"projects":"x","module":{"rules":[1,{"use":{"loader":3}}]}}"#,
    ];
    let mut plugins = IndexMap::new();
    for id in ADR_IDS {
        plugins.insert(id.to_string(), PluginConfig::Enabled(true));
    }
    for content in contents {
        let files: Vec<(&str, &str)> = config_files.iter().map(|path| (*path, content)).collect();
        let results = resolve_with(&files, &[], true, &plugins);
        assert_eq!(results.len(), ADR_IDS.len());
    }
    let broken_manifest = [(
        "package.json",
        r#"{"name":1,"eslintConfig":5,"jest":[],"lint-staged":"x","husky":null}"#,
    )];
    let results = resolve_with(&broken_manifest, &[], true, &plugins);
    assert_eq!(results.len(), ADR_IDS.len());
}

fn alias(find: &str, target: Option<&str>) -> skif_js_plugins::PathAlias {
    skif_js_plugins::PathAlias {
        find: find.to_owned(),
        target: target.map(str::to_owned),
    }
}

#[test]
fn plugins_enabled_in_an_ancestor_apply_to_the_child() {
    let files = [(
        "vitest.config.ts",
        "export default { test: { setupFiles: ['./setup.ts'] } }",
    )];
    let none = resolve_inheriting(&files, &[], &[], false, &IndexMap::new());
    assert!(none.iter().all(|(id, _)| *id != "vitest"), "{none:?}");
    let inherited = resolve_inheriting(&files, &[], &["vitest"], false, &IndexMap::new());
    assert_ne!(output(&inherited, "vitest").config_files, [] as [std::path::PathBuf; 0]);
    let prefixed = resolve_inheriting(&[], &[], &["@storybook/react"], false, &IndexMap::new());
    output(&prefixed, "storybook");
}

#[test]
fn vite_and_webpack_aliases_are_extracted() {
    let results = resolve(
        &[(
            "vite.config.ts",
            "export default { resolve: { alias: { '@': './src', '~lib': '/lib', '$': path.resolve(root, 'x'), react: 'preact/compat' } } }",
        )],
        &["vite"],
    );
    let vite = output(&results, "vite");
    assert!(vite.aliases.contains(&alias("@", Some("src"))), "{:?}", vite.aliases);
    assert!(vite.aliases.contains(&alias("~lib", Some("lib"))), "{:?}", vite.aliases);
    assert!(!vite.aliases.iter().any(|a| a.find == "react"), "{:?}", vite.aliases);

    let results = resolve(
        &[(
            "vite.config.ts",
            "export default { resolve: { alias: [{ find: '@app', replacement: './src/app' }, { find: 'dyn', replacement: compute() }] } }",
        )],
        &["vite"],
    );
    let vite = output(&results, "vite");
    assert!(
        vite.aliases.contains(&alias("@app", Some("src/app"))),
        "{:?}",
        vite.aliases
    );
    assert!(vite.aliases.contains(&alias("dyn", None)), "{:?}", vite.aliases);

    let results = resolve(
        &[(
            "webpack.config.js",
            "module.exports = { resolve: { alias: { Utils$: './src/utils' } } }",
        )],
        &["webpack"],
    );
    assert!(
        output(&results, "webpack")
            .aliases
            .contains(&alias("Utils", Some("src/utils")))
    );
}

#[test]
fn tsconfig_paths_outdir_and_conditions() {
    let results = resolve(
        &[(
            "tsconfig.json",
            r#"{"extends":"./missing.json","compilerOptions":{"baseUrl":"src","paths":{"@app/*":["lib/*"],"@root":["../index.ts"]},"outDir":"dist","rootDir":"src","customConditions":["source"]}}"#,
        )],
        &["typescript"],
    );
    let ts = output(&results, "typescript");
    assert!(ts.aliases.contains(&alias("@app", Some("src/lib"))), "{:?}", ts.aliases);
    assert!(
        ts.aliases.contains(&alias("@root", Some("index.ts"))),
        "{:?}",
        ts.aliases
    );
    assert_eq!(
        ts.build_dirs,
        [skif_js_plugins::BuildDir {
            out_dir: "dist".to_owned(),
            root_dir: "src".to_owned(),
        }]
    );
    assert_eq!(ts.conditions, ["source"]);
}

#[test]
fn bundler_resolve_extensions_are_collected() {
    let webpack = resolve(
        &[(
            "webpack.config.js",
            "module.exports = { resolve: { extensions: ['.ts', '...', '.page', 'bare'] } };",
        )],
        &["webpack"],
    );
    assert_eq!(output(&webpack, "webpack").extensions, [".ts", ".page"]);
    let vite = resolve(
        &[(
            "vite.config.ts",
            "export default defineConfig({ resolve: { extensions: ['.mjs', '.custom'] } });",
        )],
        &["vite"],
    );
    assert_eq!(output(&vite, "vite").extensions, [".mjs", ".custom"]);
}

#[test]
fn rspack_computes_paths_and_reads_html_templates() {
    let results = resolve(
        &[
            (
                "rspack.config.js",
                r"const path = require('path');
                const { resolve } = path;
                module.exports = {
                  entry: { main: path.resolve(__dirname, 'src', 'index.js'), extra: resolve(__dirname, 'src/extra.js') },
                  resolve: {
                    modules: [path.resolve(__dirname, 'src'), 'node_modules'],
                    alias: { '@ui': path.join(__dirname, 'ui') },
                  },
                };",
            ),
            (
                "public/index.html",
                r#"<script src="/static/boot.js"></script><script src="https://cdn.dev/x.js"></script>"#,
            ),
        ],
        &["@rspack/core"],
    );
    let out = output(&results, "rspack");
    assert_entries(&out.production_entries, &["src/index.js", "src/extra.js"]);
    assert_entries(&out.entries, &["static/boot.js"]);
    assert_eq!(out.module_dirs, ["src"]);
    assert!(out.aliases.contains(&alias("@ui", Some("ui"))), "{:?}", out.aliases);
}

#[test]
fn jsconfig_base_url_and_wildcard_paths_become_lookup_directories() {
    let results = resolve(
        &[(
            "jsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"*":["../shared/*","./*","./node_modules/*","./src/*"],"@kit":["../shared/kit"]},"jsx":"react-jsx"}}"#,
        )],
        &[],
    );
    let ts = output(&results, "typescript");
    assert_eq!(ts.module_dirs, ["../shared", "", "node_modules", "src"]);
    // The JSX runtime is credited per file that contains JSX, not by the compiler option.
    assert!(
        !deps(ts).contains("react"),
        "jsx: react-jsx must not add a react dependency"
    );
    assert!(
        ts.aliases.contains(&alias("@kit", Some("../shared/kit"))),
        "{:?}",
        ts.aliases
    );
}

#[test]
fn enabled_plugins_without_config_files_use_conventions() {
    let results = resolve(
        &[("index.html", r#"<script type="module" src="/src/main.tsx"></script>"#)],
        &["vite"],
    );
    assert_entries(&output(&results, "vite").entries, &["src/main.tsx"]);
    let results = resolve(&[], &["@storybook/react"]);
    assert_entries(
        &output(&results, "storybook").entries,
        &["**/*.{mdx,stories.{mdx,js,jsx,mjs,ts,tsx}}"],
    );
}

#[test]
fn tsdown() {
    let config = r#"export default defineConfig({
      entry: ["src/index.ts", "!src/skip.ts"],
      deps: { neverBundle: ["lodash"] },
    });"#;
    let results = resolve(&[("tsdown.config.ts", config)], &["tsdown"]);
    let out = output(&results, "tsdown");
    assert_entries(&out.production_entries, &["src/index.ts"]);
    assert!(!out.entries.iter().any(|e| e.contains("skip")));
    assert_deps(out, &["lodash"]);
}

#[test]
fn oxlint() {
    let config = r#"{
      "extends": ["./base.json", "oxlint-config-shared"],
      "jsPlugins": ["oxlint-plugin-a", "./local-plugin.js"],
      "overrides": [{ "files": ["*.ts"], "jsPlugins": [{ "specifier": "oxlint-plugin-b" }] }]
    }"#;
    let results = resolve(&[(".oxlintrc.json", config), ("base.json", "{}")], &["oxlint"]);
    let out = output(&results, "oxlint");
    assert_deps(out, &["oxlint-config-shared", "oxlint-plugin-a", "oxlint-plugin-b"]);
    assert_entries(&out.entries, &["local-plugin.js"]);
    assert!(out.config_files.iter().any(|f| f.ends_with("base.json")));
}

#[test]
fn angular_builders_and_entries() {
    let config = r#"{
      "projects": { "web": { "architect": {
        "build": {
          "builder": "@angular-devkit/build-angular:browser",
          "options": { "main": "src/main.ts", "polyfills": ["zone.js"], "styles": ["src/styles.css", { "input": "src/theme.scss" }] },
          "configurations": { "production": { "fileReplacements": [{ "replace": "src/env.ts", "with": "src/env.prod.ts" }] } }
        },
        "test": {
          "builder": "@angular-devkit/build-angular:karma",
          "options": { "karmaConfig": "karma.conf.js", "include": ["src/**/*.spec.ts"] }
        }
      } } }
    }"#;
    let results = resolve(&[("angular.json", config)], &["@angular/cli"]);
    let out = output(&results, "angular");
    assert_deps(out, &["@angular-devkit/build-angular", "zone.js"]);
    assert_absent(out, &["karma-jasmine"]);
    assert_entries(
        &out.production_entries,
        &["src/main.ts", "src/styles.css", "src/theme.scss", "src/env.prod.ts"],
    );
    assert_entries(&out.entries, &["src/**/*.spec.ts"]);
}

#[test]
fn angular_karma_builder_without_config_uses_the_default_packages() {
    let config = r#"{ "projects": { "web": { "architect": { "test": { "builder": "@angular/build:karma" } } } } }"#;
    let results = resolve(&[("angular.json", config)], &["@angular/cli"]);
    let out = output(&results, "angular");
    assert_deps(out, &["@angular/build", "karma-jasmine", "karma-chrome-launcher"]);
    assert_entries(&out.entries, &["**/*.spec.ts"]);
}

#[test]
fn karma() {
    let config = r#"module.exports = {
      frameworks: ["jasmine"],
      plugins: ["karma-chrome-launcher"],
      basePath: "src",
      files: ["**/*.spec.ts", { pattern: "assets/*.json" }],
    };"#;
    let results = resolve(&[("karma.conf.js", config)], &["karma"]);
    let out = output(&results, "karma");
    assert_deps(out, &["karma-jasmine", "karma-chrome-launcher"]);
    assert_entries(&out.entries, &["src/**/*.spec.ts", "src/assets/*.json"]);
}

#[test]
fn esbuild() {
    let config =
        r#"export default { entryPoints: ["src/main.ts", { in: "src/worker.ts", out: "worker" }], bundle: true };"#;
    let results = resolve(&[("esbuild.config.mjs", config)], &["esbuild"]);
    assert_entries(
        &output(&results, "esbuild").production_entries,
        &["src/main.ts", "src/worker.ts"],
    );
}

#[test]
fn svelte() {
    let results = resolve(&[], &["svelte"]);
    assert_entries(&output(&results, "svelte").entries, &["svelte.config.{js,cjs,mjs,ts}"]);
}

#[test]
fn sveltekit() {
    let config = "export default { kit: { files: { lib: 'src/shared' } } };";
    let results = resolve(&[("svelte.config.js", config)], &["@sveltejs/kit"]);
    let out = output(&results, "sveltekit");
    assert!(
        out.aliases.contains(&alias("$lib", Some("src/shared"))),
        "{:?}",
        out.aliases
    );
    assert!(out.production_entries.iter().any(|e| e.starts_with("src/routes/")));
    let results = resolve(&[], &["@sveltejs/kit"]);
    let out = output(&results, "sveltekit");
    assert!(
        out.aliases.contains(&alias("$lib", Some("src/lib"))),
        "{:?}",
        out.aliases
    );
}

#[test]
fn nuxt() {
    let config = r#"export default defineNuxtConfig({
      modules: ["@nuxt/ui", ["@pinia/nuxt", {}]],
      extends: ["@org/layer", "github:org/repo"],
      css: ["~/assets/main.css", "normalize.css"],
    });"#;
    let results = resolve(&[("nuxt.config.ts", config)], &["nuxt"]);
    let out = output(&results, "nuxt");
    assert_deps(out, &["@nuxt/ui", "@pinia/nuxt", "@org/layer", "normalize.css"]);
    assert_absent(out, &["github"]);
    assert!(out.production_entries.iter().any(|e| e.starts_with("server/")));
}

#[test]
fn astro() {
    let results = resolve_with_scripts(&[], &["astro"], &[("check", "astro check && tsc")]);
    let out = output(&results, "astro");
    assert_deps(out, &["@astrojs/check"]);
    assert!(out.production_entries.iter().any(|e| e.starts_with("src/pages/")));
    let results = resolve_with_scripts(&[], &["astro"], &[("build", "astro build")]);
    assert_absent(output(&results, "astro"), &["@astrojs/check"]);
}

#[test]
fn drizzle() {
    let config =
        r#"export default defineConfig({ schema: ["./src/db/schema.ts", "./src/db/other.ts"], out: "./drizzle" });"#;
    let results = resolve(&[("drizzle.config.ts", config)], &["drizzle-kit"]);
    assert_entries(
        &output(&results, "drizzle").production_entries,
        &["src/db/schema.ts", "src/db/other.ts"],
    );
}

#[test]
fn ts_node() {
    let results = resolve(&[], &["ts-node"]);
    assert!(
        output(&results, "ts-node")
            .provided_binaries
            .contains(&"ts-node-esm".to_string())
    );
}

#[test]
fn tsx() {
    let results = resolve_with_scripts(&[], &["tsx"], &[("test", "tsx --test src/")]);
    let out = output(&results, "tsx");
    assert!(out.provided_binaries.contains(&"tsx".to_string()));
    assert_entries(&out.entries, &["**/test/**/*.{c,m,}{j,t}s"]);
    let results = resolve_with_scripts(&[], &["tsx"], &[("dev", "tsx watch src/index.ts")]);
    assert_eq!(output(&results, "tsx").entries, [] as [std::string::String; 0]);
}

#[test]
fn nx() {
    let nx_json = r#"{
      "targetDefaults": { "@nx/vite:build": {}, "build": {} },
      "plugins": [{ "plugin": "@nx/eslint/plugin" }, "@nx/jest/plugin"],
      "generators": { "@nx/react": {} }
    }"#;
    let project = r#"{ "targets": {
      "serve": { "executor": "nx:run-commands", "options": { "commands": ["tsx scripts/a.ts", { "command": "prettier --check ." }] } },
      "lint": { "executor": "@nx/eslint:lint" }
    } }"#;
    let results = resolve(&[("nx.json", nx_json), ("project.json", project)], &["nx"]);
    let out = output(&results, "nx");
    assert_deps(out, &["@nx/vite", "@nx/eslint", "@nx/jest", "@nx/react", "nx"]);
    assert!(
        out.referenced_binaries.contains(&"tsx".to_string()),
        "{:?}",
        out.referenced_binaries
    );
}

#[test]
fn cypress() {
    let config = r#"export default defineConfig({
      e2e: { specPattern: "tests/**/*.cy.ts", supportFile: false },
      component: { specPattern: ["src/**/*.cy.tsx"] },
    });"#;
    let results = resolve(&[("cypress.config.ts", config)], &["cypress"]);
    assert_entries(
        &output(&results, "cypress").entries,
        &[
            "tests/**/*.cy.ts",
            "src/**/*.cy.tsx",
            "cypress/e2e/**/*.cy.{js,jsx,ts,tsx}",
        ],
    );
}

#[test]
fn mocha() {
    let config = r#"{
      "spec": ["test/**/*.spec.js"],
      "require": ["ts-node/register", "./setup.js"],
      "reporter": "mocha-junit-reporter"
    }"#;
    let results = resolve(&[(".mocharc.json", config)], &["mocha"]);
    let out = output(&results, "mocha");
    assert_deps(out, &["ts-node", "mocha-junit-reporter"]);
    assert_entries(&out.entries, &["test/**/*.spec.js", "setup.js"]);
    let builtin = resolve(&[(".mocharc.json", r#"{"reporter":"spec"}"#)], &["mocha"]);
    assert!(deps(output(&builtin, "mocha")).is_empty());
}

#[test]
fn prisma() {
    let manifest = r#"{"name":"app","prisma":{"schema":"db/schema","seed":"tsx db/seed.ts"}}"#;
    let results = resolve(&[("package.json", manifest)], &["prisma"]);
    let out = output(&results, "prisma");
    assert_entries(&out.entries, &["db/schema/**/*.prisma", "prisma/schema.prisma"]);
    assert!(
        out.referenced_binaries.contains(&"tsx".to_string()),
        "{:?}",
        out.referenced_binaries
    );
    let config = r#"export default { schema: "prisma/main.prisma" };"#;
    let results = resolve(&[("prisma.config.ts", config)], &["@prisma/client"]);
    assert_entries(&output(&results, "prisma").entries, &["prisma/main.prisma"]);
}

#[test]
fn graphql_codegen() {
    let config = r#"export default {
      generates: {
        "src/gql/": { preset: "client", plugins: ["typescript-operations", "@scope/custom-plugin"] },
        "schema.graphql": { plugins: [{ "schema-ast": {} }] },
        "types.ts": ["typescript"],
      },
    };"#;
    let results = resolve(&[("codegen.ts", config)], &["@graphql-codegen/cli"]);
    let out = output(&results, "graphql-codegen");
    assert_deps(
        out,
        &[
            "@graphql-codegen/client-preset",
            "@graphql-codegen/typescript-operations",
            "@scope/custom-plugin",
            "@graphql-codegen/schema-ast",
            "@graphql-codegen/typescript",
        ],
    );
    assert_entries(&out.production_entries, &["src/gql/**", "types.ts"]);
}

#[test]
fn docusaurus() {
    let config = r#"export default {
      presets: [["classic", {}]],
      plugins: ["@docusaurus/plugin-content-blog", "my-thing", "./local-plugin.js"],
      themes: ["@docusaurus/theme-mermaid"],
    };"#;
    let results = resolve(&[("docusaurus.config.ts", config)], &["@docusaurus/core"]);
    let out = output(&results, "docusaurus");
    assert_deps(
        out,
        &[
            "@docusaurus/preset-classic",
            "@docusaurus/plugin-content-blog",
            "docusaurus-plugin-my-thing",
            "@docusaurus/theme-mermaid",
        ],
    );
    assert_entries(&out.entries, &["local-plugin.js"]);
}

#[test]
fn vitepress() {
    let results = resolve(&[], &["vitepress"]);
    assert_entries(
        &output(&results, "vitepress").entries,
        &["{,docs/}.vitepress/config.{js,ts,mjs,mts,cjs,cts}"],
    );
}

#[test]
fn wrangler() {
    let results = resolve(
        &[("wrangler.toml", "name = \"w\"\nmain = \"src/worker.ts\"\n")],
        &["wrangler"],
    );
    assert_entries(&output(&results, "wrangler").production_entries, &["src/worker.ts"]);
}

#[test]
fn expo() {
    let app = r#"{ "expo": { "plugins": ["expo-router", ["expo-camera", {}], "./plugins/custom.js"] } }"#;
    let results = resolve(&[("app.json", app)], &["expo"]);
    let out = output(&results, "expo");
    assert_deps(out, &["expo-router", "expo-camera"]);
    assert_entries(&out.entries, &["plugins/custom.js"]);
}

#[test]
fn expo_router_makes_the_app_directory_the_routes() {
    let results = resolve(&[], &["expo-router"]);
    assert_entries(
        &output(&results, "expo-router").production_entries,
        &["app/**/*.{js,jsx,ts,tsx}", "src/app/**/*.{js,jsx,ts,tsx}"],
    );
    let app = r#"{ "expo": { "plugins": [["expo-router", { "root": "./screens/" }], "expo-font"] } }"#;
    let results = resolve(&[("app.json", app)], &["expo-router"]);
    assert_entries(
        &output(&results, "expo-router").production_entries,
        &["screens/**/*.{js,jsx,ts,tsx}"],
    );
    assert_eq!(
        output(&results, "expo-router").production_entries.len(),
        1,
        "a `root` replaces the default routes directories"
    );
    let plain = r#"{ "expo": { "plugins": ["expo-router"] } }"#;
    let results = resolve(&[("app.json", plain)], &["expo-router"]);
    assert_entries(
        &output(&results, "expo-router").production_entries,
        &["app/**/*.{js,jsx,ts,tsx}", "src/app/**/*.{js,jsx,ts,tsx}"],
    );
    let results = resolve(&[], &["expo"]);
    assert!(results.iter().all(|(id, _)| *id != "expo-router"));
}

#[test]
fn tanstack_router_keeps_the_route_tree_the_router_and_the_routes() {
    let results = resolve(&[], &["@tanstack/react-start"]);
    assert_entries(
        &output(&results, "tanstack-router").production_entries,
        &[
            "src/routeTree.gen.{ts,js}",
            "src/{router,start,client,server}.{js,jsx,ts,tsx}",
        ],
    );
    let config = r#"{ "generatedRouteTree": "./app/tree.gen.ts", "routesDirectory": "./app/pages" }"#;
    let results = resolve(&[("tsr.config.json", config)], &["@tanstack/react-router"]);
    let entries = &output(&results, "tanstack-router").production_entries;
    assert_entries(entries, &["app/tree.gen.ts"]);
    assert!(
        entries
            .iter()
            .all(|entry| !entry.contains("routes") && !entry.contains("pages")),
        "{entries:?}"
    );
}

#[test]
fn react_native() {
    let config = r#"module.exports = {
      dependencies: { "react-native-foo": { platforms: { ios: null } } },
      platforms: { windows: { npmPackageName: "react-native-windows" } },
    };"#;
    let results = resolve(&[("react-native.config.js", config)], &["react-native"]);
    assert_deps(
        output(&results, "react-native"),
        &["react-native-foo", "react-native-windows"],
    );
    let results = resolve(&[], &["react-native"]);
    assert_deps(output(&results, "react-native"), &["@react-native-community/cli"]);
}

#[test]
fn metro() {
    let results = resolve(&[], &["@react-native/metro-config"]);
    assert_entries(
        &output(&results, "metro").production_entries,
        &["src/**/*.{ios,android,windows,web}.{js,jsx,json,ts,tsx}"],
    );
}

#[test]
fn release_it() {
    let config = r#"{
      "plugins": { "@release-it/conventional-changelog": {} },
      "hooks": { "before:init": ["tsx scripts/check.ts"], "after:release": "echo done" },
      "github": { "releaseNotes": "git log --oneline" }
    }"#;
    let results = resolve(&[(".release-it.json", config)], &["release-it"]);
    let out = output(&results, "release-it");
    assert_deps(out, &["@release-it/conventional-changelog"]);
    assert!(
        out.referenced_binaries.contains(&"tsx".to_string()),
        "{:?}",
        out.referenced_binaries
    );
}

#[test]
fn semantic_release() {
    let manifest = r#"{"name":"app","release":{
      "extends": "@org/semantic-config",
      "plugins": ["@semantic-release/changelog", ["@semantic-release/git", {}], "@semantic-release/npm"]
    }}"#;
    let results = resolve(&[("package.json", manifest)], &["semantic-release"]);
    let out = output(&results, "semantic-release");
    assert_deps(
        out,
        &[
            "@org/semantic-config",
            "@semantic-release/changelog",
            "@semantic-release/git",
        ],
    );
    assert_absent(out, &["@semantic-release/npm"]);
}

#[test]
fn lefthook() {
    let config = "pre-commit:\n  commands:\n    lint:\n      run: oxlint --fix {staged_files}\n";
    let results = resolve(&[("lefthook.yml", config)], &["lefthook"]);
    let out = output(&results, "lefthook");
    assert!(
        out.referenced_binaries.contains(&"oxlint".to_string()),
        "{:?}",
        out.referenced_binaries
    );
}

#[test]
fn simple_git_hooks() {
    let manifest = r#"{"name":"app","simple-git-hooks":{"pre-commit":"tsx scripts/lint.ts"}}"#;
    let results = resolve(&[("package.json", manifest)], &["simple-git-hooks"]);
    let out = output(&results, "simple-git-hooks");
    assert_deps(out, &["simple-git-hooks"]);
    assert!(
        out.referenced_binaries.contains(&"tsx".to_string()),
        "{:?}",
        out.referenced_binaries
    );
}

#[test]
fn stylelint() {
    let config = r#"{
      "extends": ["stylelint-config-standard", "./base.json"],
      "plugins": ["stylelint-order"],
      "customSyntax": "postcss-scss",
      "overrides": [{ "files": ["*.vue"], "extends": "stylelint-config-recommended-vue" }]
    }"#;
    let results = resolve(&[(".stylelintrc.json", config), ("base.json", "{}")], &["stylelint"]);
    let out = output(&results, "stylelint");
    assert_deps(
        out,
        &[
            "stylelint-config-standard",
            "stylelint-order",
            "postcss-scss",
            "stylelint-config-recommended-vue",
        ],
    );
    assert!(out.config_files.iter().any(|f| f.ends_with("base.json")));
}

#[test]
fn markdownlint() {
    let config = r#"{
      "config": { "extends": "markdownlint-config-x" },
      "customRules": ["markdownlint-rule-foo"],
      "markdownItPlugins": [["markdown-it-katex"]],
      "outputFormatters": [["markdownlint-cli2-formatter-default"], ["markdownlint-cli2-formatter-json"]]
    }"#;
    let results = resolve(&[(".markdownlint-cli2.jsonc", config)], &["markdownlint-cli2"]);
    let out = output(&results, "markdownlint");
    assert_deps(
        out,
        &[
            "markdownlint-config-x",
            "markdownlint-rule-foo",
            "markdown-it-katex",
            "markdownlint-cli2-formatter-json",
        ],
    );
    assert_absent(out, &["markdownlint-cli2-formatter-default"]);
}

#[test]
fn cspell() {
    let config = r#"{ "import": ["@cspell/dict-fr-fr/cspell-ext.json", "./words.json"] }"#;
    let results = resolve(&[("cspell.json", config), ("words.json", "{}")], &["cspell"]);
    let out = output(&results, "cspell");
    assert_deps(out, &["@cspell/dict-fr-fr"]);
    assert!(out.config_files.iter().any(|f| f.ends_with("words.json")));
}

#[test]
fn nodemon() {
    let config = r#"{ "exec": "tsx src/server.ts", "events": { "start": "echo hi" } }"#;
    let results = resolve(&[("nodemon.json", config)], &["nodemon"]);
    let out = output(&results, "nodemon");
    assert!(
        out.referenced_binaries.contains(&"tsx".to_string()),
        "{:?}",
        out.referenced_binaries
    );
}

#[test]
fn pm2() {
    let config = r#"module.exports = { apps: [{ name: "a", script: "./dist/server.js" }, { script: "npm" }] };"#;
    let results = resolve(&[("ecosystem.config.cjs", config)], &["pm2"]);
    let out = output(&results, "pm2");
    assert_entries(&out.entries, &["dist/server.js"]);
    assert!(!out.entries.iter().any(|e| e == "npm"));
}

#[test]
fn pnpm() {
    let workspace =
        "packages:\n  - 'pkg/*'\npackageExtensions:\n  'react-foo@*':\n    peerDependencies:\n      react: '*'\n";
    let results = resolve(&[("pnpm-workspace.yaml", workspace)], &[]);
    let out = output(&results, "pnpm");
    assert_deps(out, &["react"]);
    assert_entries(&out.entries, &[".pnpmfile.{cjs,mjs}"]);
}

#[test]
fn yarn() {
    let rc = "yarnPath: .yarn/releases/yarn-4.cjs\nplugins:\n  - path: .yarn/plugins/a.cjs\n  - ./b.cjs\n";
    let results = resolve(&[(".yarnrc.yml", rc)], &[]);
    let out = output(&results, "yarn");
    assert_entries(
        &out.entries,
        &[
            ".yarn/releases/yarn-4.cjs",
            ".yarn/plugins/a.cjs",
            "b.cjs",
            "yarn.config.cjs",
        ],
    );
}

#[test]
fn syncpack() {
    let results = resolve(&[], &["syncpack"]);
    assert_entries(&output(&results, "syncpack").entries, &["syncpack.config.{js,cjs}"]);
}

fn has_virtual(out: &PluginOutput, pattern: &str) -> bool {
    out.virtual_modules.iter().any(|candidate| candidate == pattern)
}

#[test]
fn the_node_plugin_adds_the_test_runner_entries_only_for_a_script_that_runs_it() {
    let results = resolve_with_scripts(&[], &[], &[("test", "node --test --test-reporter=spec")]);
    let out = output(&results, "node");
    assert_entries(&out.production_entries, &["server.js"]);
    assert_entries(
        &out.entries,
        &[
            "**/test/**/*.{cjs,mjs,js,cts,mts,ts}",
            "**/test-*.{cjs,mjs,js,cts,mts,ts}",
        ],
    );
    let results = resolve_with_scripts(&[], &[], &[("start", "node app.js")]);
    let out = output(&results, "node");
    assert_eq!(out.entries, ["server.js"]);
}

#[test]
fn docusaurus_declares_its_virtual_modules_and_reads_the_babel_config() {
    let files = [
        ("docusaurus.config.js", "module.exports = {};"),
        ("babel.config.js", "module.exports = { presets: ['custom'] };"),
    ];
    let results = resolve(&files, &["@docusaurus/core", "@docusaurus/preset-classic"]);
    let out = output(&results, "docusaurus");
    for pattern in ["@generated/*", "@theme/*", "@theme-original/*"] {
        assert!(has_virtual(out, pattern), "{pattern}: {:?}", out.virtual_modules);
    }
    assert!(
        out.virtual_modules
            .iter()
            .any(|pattern| pattern.starts_with("@docusaurus/{BrowserOnly,"))
    );
    assert!(out.aliases.contains(&alias("@site", Some("."))), "{:?}", out.aliases);
    assert_deps(out, &["babel-preset-custom"]);
    assert!(out.config_files.iter().any(|file| file.ends_with("babel.config.js")));

    let results = resolve(&files, &["@docusaurus/core"]);
    assert!(
        !has_virtual(output(&results, "docusaurus"), "@theme/*"),
        "no theme without the classic theme"
    );
}

#[test]
fn nuxt_declares_its_virtual_modules_and_source_aliases() {
    let results = resolve(&[("nuxt.config.ts", "export default defineNuxtConfig({});")], &["nuxt"]);
    let out = output(&results, "nuxt");
    for pattern in ["#imports", "#components", "#app", "#app/*", "#build/*"] {
        assert!(has_virtual(out, pattern), "{pattern}: {:?}", out.virtual_modules);
    }
    for find in ["~", "@"] {
        assert!(
            out.aliases.contains(&alias(find, Some("."))),
            "{find}: {:?}",
            out.aliases
        );
    }
    assert!(out.aliases.contains(&alias("~~", Some("."))));

    let results = resolve(&[("app/app.vue", "<template/>")], &["nuxt"]);
    assert!(output(&results, "nuxt").aliases.contains(&alias("~", Some("app"))));
}

#[test]
fn angular_links_the_typescript_and_karma_configs_of_its_targets() {
    let angular = r#"{ "projects": { "web": { "architect": {
      "build": {
        "builder": "@angular-devkit/build-angular:browser",
        "options": { "tsConfig": "projects/web/tsconfig.app.json" }
      },
      "test": {
        "builder": "@angular-devkit/build-angular:karma",
        "options": { "karmaConfig": "projects/web/karma.conf.js", "tsConfig": "projects/web/tsconfig.spec.json" }
      }
    } } } }"#;
    let files = [
        ("angular.json", angular),
        (
            "projects/web/tsconfig.app.json",
            r#"{"compilerOptions":{"types":["node"]}}"#,
        ),
        (
            "projects/web/tsconfig.spec.json",
            r#"{"compilerOptions":{"jsxImportSource":"preact"}}"#,
        ),
        (
            "projects/web/karma.conf.js",
            "module.exports = { frameworks: ['jasmine'] };",
        ),
    ];
    let results = resolve(&files, &["@angular/cli"]);
    let out = output(&results, "angular");
    assert_deps(out, &["@types/node", "preact", "karma-jasmine"]);
    for file in ["tsconfig.app.json", "tsconfig.spec.json", "karma.conf.js"] {
        assert!(
            out.config_files.iter().any(|path| path.ends_with(file)),
            "{file}: {:?}",
            out.config_files
        );
    }
}

#[test]
fn expo_and_metro_read_the_babel_config() {
    let files = [("babel.config.js", "module.exports = { presets: ['custom'] };")];
    let results = resolve(&files, &["expo"]);
    assert_deps(output(&results, "expo"), &["babel-preset-custom"]);
    assert_deps(output(&results, "metro"), &["babel-preset-custom"]);
}

#[test]
fn a_karma_config_function_gives_its_set_argument_to_the_plugin() {
    let config = "module.exports = function (config) {\n  config.set({ frameworks: ['jasmine'], plugins: ['karma-chrome-launcher'], \
                  basePath: 'src', files: ['**/*.spec.ts'] });\n};";
    let results = resolve(&[("karma.conf.js", config)], &["karma"]);
    let out = output(&results, "karma");
    assert_deps(out, &["karma-jasmine", "karma-chrome-launcher"]);
    assert_entries(&out.entries, &["src/**/*.spec.ts"]);
}

#[test]
fn a_linked_config_is_read_once_even_when_two_plugins_link_it() {
    let results = resolve(
        &[("babel.config.js", "module.exports = { presets: ['custom'] };")],
        &["expo", "@babel/core"],
    );
    let babel_files = &output(&results, "babel").config_files;
    assert_eq!(babel_files.len(), 1);
    let expo_files = &output(&results, "expo").config_files;
    assert_eq!(expo_files.len(), 1);
}

#[test]
fn declarative_plugins_enable_on_their_package_and_declare_entries() {
    // (plugin, enabling dependency, pattern in `entries`, pattern in `production_entries`, provided binary)
    let table = [
        (
            "unocss",
            "unocss",
            Some("{uno,unocss}.config.{json,ts,mts,cts,js,mjs,cjs}"),
            None,
            Some("unocss"),
        ),
        (
            "lingui",
            "@lingui/cli",
            Some("lingui.config.{js,cjs,ts,mjs}"),
            None,
            Some("lingui"),
        ),
        (
            "remix",
            "@remix-run/dev",
            Some("remix.config.js"),
            Some("app/root.tsx"),
            None,
        ),
        (
            "svgo",
            "@svgr/plugin-svgo",
            Some("svgo.config.{js,cjs,mjs}"),
            None,
            Some("svgo"),
        ),
        (
            "sentry",
            "@sentry/nextjs",
            None,
            Some("sentry.{client,server,edge}.config.{js,ts}"),
            None,
        ),
        (
            "vercel",
            "@vercel/config",
            Some("vercel.{js,mjs,cjs,ts,mts}"),
            None,
            None,
        ),
        ("plop", "plop", Some("plopfile.{cjs,mjs,js,ts}"), None, Some("plop")),
        ("sanity", "sanity", Some("sanity.cli.{ts,js}"), None, Some("sanity")),
        (
            "vike",
            "vike",
            None,
            Some("{pages,renderer}/**/+*.{js,jsx,ts,tsx,vue,react,solid}"),
            None,
        ),
        (
            "i18next-parser",
            "i18next-parser",
            Some("i18next-parser.config.{js,mjs,json,ts,yaml,yml}"),
            None,
            Some("i18next"),
        ),
        ("c8", "c8", None, None, Some("c8")),
        ("dotenv", "dotenv-cli", None, None, Some("dotenv")),
        ("tsd", "tsd", Some("*.test-d.{ts,tsx}"), None, Some("tsd")),
        ("msw", "msw", Some("mockServiceWorker.js"), None, Some("msw")),
    ];
    for (id, dependency, entry, production, binary) in table {
        let results = resolve(&[], &[dependency]);
        let out = output(&results, id);
        if let Some(entry) = entry {
            assert_entries(&out.entries, &[entry]);
        }
        if let Some(production) = production {
            assert_entries(&out.production_entries, &[production]);
        }
        if let Some(binary) = binary {
            assert!(out.provided_binaries.contains(&binary.to_string()), "{id}: {out:?}");
        }
        let enabled: Vec<PluginId> = resolve(&[], &[]).into_iter().map(|(plugin, _)| plugin).collect();
        assert!(!enabled.contains(&id), "{id} must not be enabled without {dependency}");
    }
}

#[test]
fn rsbuild() {
    let config = r"export default defineConfig({
      source: { entry: { index: './src/index.ts', admin: { import: ['./src/admin.ts'] } }, preEntry: './src/pre.ts' },
      resolve: { alias: { '@': './src' } },
      environments: { node: { source: { entry: { server: './src/server.ts' } } } },
    });";
    let results = resolve(&[("rsbuild.config.ts", config)], &["@rsbuild/core"]);
    let out = output(&results, "rsbuild");
    assert_entries(
        &out.production_entries,
        &["src/index.ts", "src/admin.ts", "src/pre.ts", "src/server.ts"],
    );
    assert!(out.aliases.iter().any(|a| a.find == "@"), "{:?}", out.aliases);
}

#[test]
fn rslib() {
    let config = "export default defineConfig({ lib: [{ source: { entry: { index: './src/index.ts' } } }] });";
    let results = resolve(&[("rslib.config.ts", config)], &["@rslib/core"]);
    assert_entries(&output(&results, "rslib").production_entries, &["src/index.ts"]);
}

#[test]
fn rolldown() {
    let config = "export default { input: ['src/main.js', 'src/worker.js'], output: { dir: 'dist' } };";
    let results = resolve(&[("rolldown.config.js", config)], &["rolldown"]);
    assert_entries(
        &output(&results, "rolldown").production_entries,
        &["src/main.js", "src/worker.js"],
    );
}

#[test]
fn unbuild() {
    let config = "export default defineBuildConfig({ entries: ['src/index', { input: 'src/cli/', outDir: 'dist' }] });";
    let results = resolve(&[("build.config.ts", config)], &["unbuild"]);
    assert_entries(&output(&results, "unbuild").entries, &["src/index", "src/cli"]);
}

#[test]
fn stryker() {
    let config = r#"{
      "testRunner": "vitest",
      "checkers": ["typescript"],
      "plugins": ["@stryker-mutator/vitest-runner", "@stryker-mutator/*"]
    }"#;
    let results = resolve(&[("stryker.config.json", config)], &["@stryker-mutator/core"]);
    let out = output(&results, "stryker");
    assert_deps(
        out,
        &["@stryker-mutator/vitest-runner", "@stryker-mutator/typescript-checker"],
    );
    assert_absent(out, &["@stryker-mutator/*"]);
}

#[test]
fn typedoc() {
    let config = r#"{
      "plugin": ["typedoc-plugin-markdown", "./local-plugin.js"],
      "theme": "typedoc-theme-x",
      "customCss": "docs/extra.css"
    }"#;
    let results = resolve(&[("typedoc.json", config)], &["typedoc"]);
    let out = output(&results, "typedoc");
    assert_deps(out, &["typedoc-plugin-markdown", "typedoc-theme-x"]);
    assert_entries(&out.entries, &["local-plugin.js", "docs/extra.css"]);

    let tsconfig = r#"{ "compilerOptions": {}, "typedocOptions": { "plugin": "typedoc-plugin-mdn-links" } }"#;
    let results = resolve(&[("tsconfig.json", tsconfig)], &["typedoc"]);
    assert_deps(output(&results, "typedoc"), &["typedoc-plugin-mdn-links"]);

    let manifest = r#"{ "name": "app", "typedocOptions": { "theme": "typedoc-theme-y" } }"#;
    let results = resolve(&[("package.json", manifest)], &["typedoc"]);
    assert_deps(output(&results, "typedoc"), &["typedoc-theme-y"]);
}

#[test]
fn react_router_reads_the_route_modules_of_the_app_directory() {
    let routes = "import { index, route } from \"@react-router/dev/routes\";\n// route(\"x\", \"routes/commented.tsx\")\n\
                  export default [index(\"routes/home.tsx\"), route(\"about\", \"./routes/about.tsx\")];";
    let results = resolve(
        &[
            (
                "react-router.config.ts",
                "export default { appDirectory: 'src/app' } satisfies Config;",
            ),
            ("src/app/routes.ts", routes),
        ],
        &["@react-router/dev"],
    );
    let out = output(&results, "react-router");
    assert_entries(
        &out.production_entries,
        &[
            "src/app/root.{jsx,tsx}",
            "src/app/routes/home.tsx",
            "src/app/routes/about.tsx",
        ],
    );
    assert!(!out.production_entries.iter().any(|e| e.contains("commented")));
    assert_entries(&out.entries, &["src/app/routes.{js,ts}"]);
    assert_deps(out, &["@react-router/node", "isbot"]);
}

#[test]
fn react_router_without_a_config_uses_the_app_directory() {
    let results = resolve(&[("app/entry.server.tsx", "export default 1;")], &["@react-router/dev"]);
    let out = output(&results, "react-router");
    assert_entries(&out.production_entries, &["app/entry.{client,server}.{js,jsx,ts,tsx}"]);
    assert_absent(out, &["@react-router/node", "isbot"]);
}

#[test]
fn electron_vite() {
    let config = "export default defineConfig({ main: { build: { rollupOptions: { input: { index: 'electron/main.ts' } } } }, renderer: {} });";
    let html = "<html><script type=\"module\" src=\"./renderer.ts\"></script></html>";
    let results = resolve(
        &[("electron.vite.config.ts", config), ("src/renderer/index.html", html)],
        &["electron-vite"],
    );
    let out = output(&results, "electron-vite");
    assert_entries(
        &out.production_entries,
        &[
            "electron/main.ts",
            "src/preload/index.{js,mjs,cjs,ts,mts,cts}",
            "src/renderer/index.html",
            "src/renderer/renderer.ts",
        ],
    );
    assert_entries(
        &out.production_entries,
        &["src/main/index.{js,mjs,cjs,ts,mts,cts}", "src/renderer/index.html"],
    );
}

#[test]
fn gatsby() {
    let config =
        "module.exports = { plugins: ['gatsby-plugin-image', { resolve: 'gatsby-source-filesystem', options: {} }] };";
    let results = resolve(&[("gatsby-config.js", config)], &["gatsby"]);
    let out = output(&results, "gatsby");
    assert_deps(out, &["gatsby-plugin-image", "gatsby-source-filesystem"]);
    assert_entries(&out.production_entries, &["src/pages/**/*.{js,jsx,ts,tsx}"]);
}

#[test]
fn netlify() {
    let config = "[functions]\ndirectory = \"api/fns\"\nincluded_files = [\"data/**\", \"!data/skip\"]\n\
                  external_node_modules = [\"sharp\"]\n\n[functions.\"resize\"]\nincluded_files = [\"fonts/*\"]\n\n\
                  [[plugins]]\npackage = \"@netlify/plugin-nextjs\"\n";
    let results = resolve(&[("netlify.toml", config)], &["netlify-cli"]);
    let out = output(&results, "netlify");
    assert_entries(
        &out.production_entries,
        &["api/fns/**/*.{js,mjs,cjs,ts,mts,cts}", "data/**", "fonts/*"],
    );
    assert!(!out.production_entries.iter().any(|e| e.contains("skip")));
    assert_deps(out, &["sharp", "@netlify/plugin-nextjs"]);
}

#[test]
fn webdriver_io_reads_a_named_config_export() {
    let config = "import type { Options } from '@wdio/types';\nexport const config: Options.Testrunner = {\n  \
                  framework: 'mocha', runner: 'local', reporters: ['spec', ['allure', {}]],\n};";
    let results = resolve(&[("wdio.conf.ts", config)], &["@wdio/cli"]);
    let out = output(&results, "webdriver-io");
    assert_deps(
        out,
        &["@wdio/mocha-framework", "@wdio/spec-reporter", "@wdio/allure-reporter"],
    );
    assert_absent(out, &["@wdio/local-runner"]);

    let results = resolve(
        &[(
            "wdio.conf.js",
            "exports.config = { framework: 'jasmine', runner: 'browser' };",
        )],
        &["@wdio/cli"],
    );
    assert_deps(
        output(&results, "webdriver-io"),
        &["@wdio/jasmine-framework", "@wdio/browser-runner"],
    );
}

#[test]
fn nyc() {
    let config = r#"{ "extends": "@istanbuljs/nyc-config-typescript", "require": ["ts-node/register", "./setup.js"] }"#;
    let results = resolve(&[(".nycrc", config)], &["nyc"]);
    let out = output(&results, "nyc");
    assert_deps(out, &["@istanbuljs/nyc-config-typescript", "ts-node"]);
    assert_entries(&out.entries, &["setup.js"]);
}

#[test]
fn knex() {
    let config = "module.exports = { development: { client: 'pg', migrations: { directory: './db/migrations' }, \
                  seeds: { directory: ['./db/seeds'] } } };";
    let results = resolve(&[("knexfile.js", config)], &["knex"]);
    let out = output(&results, "knex");
    assert_deps(out, &["pg"]);
    assert_entries(&out.entries, &["db/migrations/*.{js,ts}", "db/seeds/*.{js,ts}"]);
}

#[test]
fn nest() {
    let results = resolve(
        &[("nest-cli.json", r#"{ "collection": "@nestjs/schematics" }"#)],
        &["@nestjs/core"],
    );
    assert_deps(output(&results, "nest"), &["@nestjs/schematics"]);
}

#[test]
fn oclif() {
    let manifest =
        r#"{ "name": "app", "oclif": { "plugins": ["@oclif/plugin-help"], "devPlugins": ["@oclif/plugin-plugins"] } }"#;
    let results = resolve(&[("package.json", manifest)], &["@oclif/core"]);
    let out = output(&results, "oclif");
    assert_deps(out, &["@oclif/plugin-help", "@oclif/plugin-plugins"]);
    assert_entries(
        &out.production_entries,
        &["{,src/}commands/**/*.{js,mjs,cjs,jsx,ts,tsx,mts,cts}"],
    );
}

#[test]
fn panda_css() {
    let config = "export default defineConfig({ presets: ['@pandacss/preset-base', '@acme/preset', somePreset] });";
    let results = resolve(&[("panda.config.ts", config)], &["@pandacss/dev"]);
    assert_deps(
        output(&results, "panda-css"),
        &["@pandacss/preset-base", "@acme/preset", "postcss"],
    );
}

#[test]
fn preconstruct() {
    let manifest = r#"{ "name": "app", "preconstruct": { "entrypoints": ["index.{js,ts}", "utils.{js,ts}"] } }"#;
    let results = resolve(&[("package.json", manifest)], &["@preconstruct/cli"]);
    assert_entries(
        &output(&results, "preconstruct").production_entries,
        &["src/index.{js,ts}", "src/utils.{js,ts}"],
    );
}

#[test]
fn remark_resolves_plugins_by_convention_and_by_full_name() {
    let manifest = r#"{ "name": "app", "remarkConfig": { "plugins": ["preset-lint-recommended", ["lint-no-dead-urls", {}], "gfm", "lint-foo", "./local.js"] } }"#;
    let results = resolve(&[("package.json", manifest)], &["remark-cli", "lint-foo"]);
    let out = output(&results, "remark");
    assert_deps(
        out,
        &[
            "remark-preset-lint-recommended",
            "remark-lint-no-dead-urls",
            "remark-gfm",
            "lint-foo",
        ],
    );
    assert_absent(out, &["remark-lint-foo"]);
    assert_entries(&out.entries, &["local.js"]);
}

#[test]
fn swc() {
    let config =
        r#"{ "jsc": { "externalHelpers": true, "experimental": { "plugins": [["@swc/plugin-emotion", {}]] } } }"#;
    let results = resolve(&[(".swcrc", config)], &["@swc/core"]);
    assert_deps(output(&results, "swc"), &["@swc/helpers", "@swc/plugin-emotion"]);
}

#[test]
fn svgr() {
    let results = resolve(
        &[(".svgrrc", r#"{ "plugins": ["@svgr/plugin-svgo", "@svgr/plugin-jsx"] }"#)],
        &["@svgr/core"],
    );
    assert_deps(output(&results, "svgr"), &["@svgr/plugin-svgo", "@svgr/plugin-jsx"]);
}

#[test]
fn biome() {
    let results = resolve(
        &[
            (
                "biome.json",
                r#"{ "extends": ["./base.json", "@org/biome-config", "//"] }"#,
            ),
            ("base.json", "{}"),
        ],
        &["@biomejs/biome"],
    );
    let out = output(&results, "biome");
    assert_deps(out, &["@org/biome-config"]);
    assert!(out.config_files.iter().any(|f| f.ends_with("base.json")));
}

#[test]
fn nitro() {
    let config = "export default defineNitroConfig({ srcDir: 'server', apiDir: 'endpoints', \
                  modules: ['nitro-module-x', ['nitro-module-y', {}]] });";
    let results = resolve(&[("nitro.config.ts", config)], &["nitropack"]);
    let out = output(&results, "nitro");
    assert_entries(
        &out.production_entries,
        &[
            "server/endpoints/**/*.ts",
            "server/server.{js,mjs,ts}",
            "server/plugins/**/*.ts",
        ],
    );
    assert_deps(out, &["nitro-module-x", "nitro-module-y"]);
    assert!(has_virtual(out, "#nitro-internal-*"));
}

#[test]
fn tap() {
    let manifest = (
        "package.json",
        r#"{"tap":{"before":"test/setup.js","after":"test/fixtures/cleanup.js"}}"#,
    );
    let results = resolve(&[manifest], &["tap"]);
    let out = output(&results, "tap");
    assert_entries(
        &out.entries,
        &[
            "{test,tests,__tests__}/**/*.{js,cjs,mjs,jsx,ts,cts,mts,tsx}",
            "**/*.{test,spec}.{js,cjs,mjs,jsx,ts,cts,mts,tsx}",
            "**/test.{js,cjs,mjs,jsx,ts,cts,mts,tsx}",
            "test/setup.js",
            "test/fixtures/cleanup.js",
        ],
    );
}

#[test]
fn bun_is_enabled_by_a_test_script_or_a_lockfile() {
    let results = resolve_with_scripts(&[], &[], &[("test", "bun test --timeout 30000")]);
    let out = output(&results, "bun");
    assert_entries(
        &out.entries,
        &["**/*.{test,spec}.{js,jsx,ts,tsx}", "**/*_{test,spec}.{js,jsx,ts,tsx}"],
    );
    assert!(resolve(&[("bun.lock", "")], &[]).iter().any(|(id, _)| *id == "bun"));
    assert!(
        !resolve_with_scripts(&[], &[], &[("test", "vitest run")])
            .iter()
            .any(|(id, _)| *id == "bun")
    );
}

#[test]
fn bun_reads_preloads_from_bunfig() {
    let config = "[test]\npreload = [\"./test/setup.ts\", \"happy-dom-setup\"]\n";
    let results = resolve(&[("bun.lock", ""), ("bunfig.toml", config)], &[]);
    let out = output(&results, "bun");
    assert_entries(&out.entries, &["test/setup.ts"]);
    assert_deps(out, &["happy-dom-setup"]);
}

#[test]
fn oxfmt() {
    let results = resolve(&[("oxfmt.config.ts", "export default {};")], &["oxfmt"]);
    let out = output(&results, "oxfmt");
    assert!(out.config_files.iter().any(|path| path.ends_with("oxfmt.config.ts")));
}

#[test]
fn jest_runner_named_in_full_is_not_prefixed() {
    let config = "export default { runner: 'jest-light-runner' };";
    let results = resolve(&[("jest.config.ts", config)], &["jest"]);
    assert_deps(output(&results, "jest"), &["jest-light-runner"]);
}

#[test]
fn ava() {
    let config =
        "export default { files: ['src/**/*.test.ts', '!src/skip.test.ts'], require: ['./setup.cjs', 'tsx'] };";
    let results = resolve(&[("ava.config.mjs", config)], &["ava"]);
    let out = output(&results, "ava");
    assert_entries(
        &out.entries,
        &[
            "src/**/*.test.ts",
            "!src/skip.test.ts",
            "setup.cjs",
            "**/*.spec.{js,cjs,mjs,ts}",
        ],
    );
    assert_deps(out, &["tsx"]);
}

#[test]
fn ava_reads_node_arguments_like_a_node_command() {
    let config = "export default { nodeArguments: ['--import=tsx', '--loader=ts-node/esm'] };";
    let results = resolve(&[("ava.config.mjs", config)], &["ava"]);
    assert_deps(output(&results, "ava"), &["tsx", "ts-node"]);
}

#[test]
fn jasmine_resolves_paths_against_the_spec_directory() {
    let config = r#"{
      "spec_dir": "spec",
      "spec_files": ["**/*[sS]pec.js", "!**/skip.js"],
      "helpers": ["helpers/**/*.js"],
      "requires": ["ts-node/register"]
    }"#;
    let results = resolve(&[("spec/support/jasmine.json", config)], &["jasmine"]);
    let out = output(&results, "jasmine");
    assert_entries(
        &out.entries,
        &["spec/**/*[sS]pec.js", "spec/helpers/**/*.js", "!spec/**/skip.js"],
    );
    assert_deps(out, &["ts-node"]);
}

#[test]
fn jasmine_reads_its_command_line() {
    let scripts = [(
        "test",
        "jasmine --fail-fast --config spec/support/ci.json spec/unit/a.spec.js init NODE_ENV=test --require ts-node/register",
    )];
    let results = resolve_with_scripts(&[], &["jasmine"], &scripts);
    let out = output(&results, "jasmine");
    assert_entries(&out.entries, &["spec/unit/a.spec.js"]);
    assert!(
        !out.entries.iter().any(|e| e == "init" || e.contains("NODE_ENV")),
        "{:?}",
        out.entries
    );
    assert_deps(out, &["ts-node"]);
}

#[test]
fn msw_reads_the_worker_directory() {
    let manifest = r#"{ "name": "app", "msw": { "workerDirectory": ["public"] } }"#;
    let results = resolve(&[("package.json", manifest)], &["msw"]);
    assert_entries(&output(&results, "msw").entries, &["public/mockServiceWorker.js"]);
}

#[test]
fn tsd_reads_its_directory() {
    let manifest = r#"{ "name": "app", "tsd": { "directory": "types-test" } }"#;
    let results = resolve(&[("package.json", manifest)], &["tsd"]);
    assert_entries(&output(&results, "tsd").entries, &["types-test/**/*.test-d.{ts,tsx}"]);
}

#[test]
fn size_limit_uses_its_presets() {
    let results = resolve(&[], &["size-limit", "@size-limit/preset-small-lib", "@size-limit/file"]);
    let out = output(&results, "size-limit");
    assert_deps(out, &["@size-limit/preset-small-lib", "@size-limit/file"]);
    assert_entries(&out.entries, &["size-limit.{json,js,cjs,mjs,ts,mts,cts}"]);
}

#[test]
fn knex_reads_the_client_of_calls_in_source_files() {
    let files = [
        (
            "src/db.ts",
            "import knex from 'knex';\nexport const db = knex({ client: 'mysql2', connection: { client: 'x' } });",
        ),
        (
            "src/named.ts",
            "import { knex as connect } from \"knex\";\nconnect({ connection: {}, client: \"sqlite3\" });",
        ),
        (
            "src/cjs.js",
            "const k = require('knex');\nconst { knex: build } = require('knex');\nk({ client: 'pg' });\nbuild({ client: 'oracledb' });",
        ),
        (
            "src/other.ts",
            "import knex from 'knex';\nknex({ client: dialect });\nunrelated({ client: 'mssql' });",
        ),
    ];
    let results = resolve(&files, &["knex"]);
    let out = output(&results, "knex");
    assert_deps(out, &["mysql2", "sqlite3", "pg", "oracledb"]);
    assert_absent(out, &["tedious", "x"]);
    assert!(
        out.referenced_dependencies.iter().all(|d| !d.optional),
        "a driver imported by a call is a dependency: {:?}",
        out.referenced_dependencies
    );
}

#[test]
fn react_router_reads_flat_routes_and_literal_file_names() {
    let routes = "import { flatRoutes } from \"@react-router/fs-routes\";\nexport default flatRoutes({ rootDirectory: \"pages\" });\n\
                  // route(\"x\", \"routes/$id.tsx\");\nconst more = [route(\"p\", \"routes/$id.[lang].tsx\")];";
    let results = resolve(&[("app/routes.ts", routes)], &["@react-router/dev"]);
    let out = output(&results, "react-router");
    assert_entries(
        &out.production_entries,
        &[
            "app/pages/*.{js,jsx,ts,tsx,md,mdx}",
            "app/pages/*/{route,index}.{js,jsx,ts,tsx,md,mdx}",
            "app/routes/$id.[[]lang[]].tsx",
        ],
    );

    let default_directory = "export default flatRoutes();";
    let results = resolve(&[("app/routes.ts", default_directory)], &["@react-router/dev"]);
    assert_entries(
        &output(&results, "react-router").production_entries,
        &["app/routes/*.{js,jsx,ts,tsx,md,mdx}"],
    );
}

#[test]
fn rsbuild_reads_swc_plugins_and_config_functions() {
    let config = "export default defineConfig(({ env }) => ({\n  source: { entry: { index: './src/index.ts' } },\n  \
                  tools: { swc: { jsc: { experimental: { plugins: [['@swc/plugin-emotion', {}], '@swc/plugin-styled-jsx'] } } } },\n  \
                  environments: { node: { tools: { swc: { jsc: { experimental: { plugins: ['@swc/plugin-relay'] } } } } } },\n}));";
    let results = resolve(&[("rsbuild.config.ts", config)], &["@rsbuild/core"]);
    let out = output(&results, "rsbuild");
    assert_entries(&out.production_entries, &["src/index.ts"]);
    assert_deps(
        out,
        &["@swc/plugin-emotion", "@swc/plugin-styled-jsx", "@swc/plugin-relay"],
    );
    assert!(out.referenced_dependencies.iter().all(|d| d.optional));
}

#[test]
fn webpack_reads_the_swc_plugins_of_an_swc_loader() {
    let config = "module.exports = { module: { rules: [{ test: /\\.ts$/, use: { loader: 'swc-loader', \
                  options: { jsc: { experimental: { plugins: [['@swc/plugin-emotion', {}]] } } } } }] } };";
    let results = resolve(&[("webpack.config.js", config)], &["webpack"]);
    assert_deps(output(&results, "webpack"), &["swc-loader", "@swc/plugin-emotion"]);
}

#[test]
fn stryker_reads_the_config_file_of_its_run_command() {
    let scripts = [
        ("mutate", "stryker run --incremental stryker.ci.conf.json"),
        ("other", "stryker init"),
    ];
    let files = [("stryker.ci.conf.json", r#"{ "testRunner": "jest" }"#)];
    let results = resolve_with_scripts(&files, &["@stryker-mutator/core"], &scripts);
    let out = output(&results, "stryker");
    assert_deps(out, &["@stryker-mutator/jest-runner"]);
    assert!(out.config_files.iter().any(|f| f.ends_with("stryker.ci.conf.json")));
}

#[test]
fn tsd_reads_files_and_the_directory_of_its_command() {
    let scripts = [
        ("types", "tsd --files 'check/**/*.test-d.ts' -f other.test-d.ts"),
        ("types:dir", "tsd typings-tests"),
    ];
    let results = resolve_with_scripts(&[], &["tsd"], &scripts);
    assert_entries(
        &output(&results, "tsd").entries,
        &[
            "check/**/*.test-d.ts",
            "other.test-d.ts",
            "typings-tests/**/*.test-d.{ts,tsx}",
        ],
    );
}

#[test]
fn typedoc_reads_plugins_and_themes_of_its_command() {
    let scripts = [(
        "docs",
        "typedoc --plugin typedoc-plugin-markdown --plugin ./local.js --theme typedoc-theme-x src/index.ts",
    )];
    let results = resolve_with_scripts(&[], &["typedoc"], &scripts);
    let out = output(&results, "typedoc");
    assert_deps(out, &["typedoc-plugin-markdown", "typedoc-theme-x"]);
    assert_entries(&out.entries, &["local.js"]);
    assert!(out.referenced_dependencies.iter().all(|d| d.optional));
}

#[test]
fn gatsby_node_names_babel_plugins() {
    let node = "exports.onCreateBabelConfig = ({ actions }) => {\n  actions.setBabelPlugin({ name: \"babel-plugin-styled-components\", options: { name: 'no' } });\n  \
                actions.setBabelPlugin({ name: require.resolve('x') });\n};";
    let results = resolve(&[("gatsby-node.js", node)], &["gatsby"]);
    let out = output(&results, "gatsby");
    assert_deps(out, &["babel-plugin-styled-components"]);
    assert_absent(out, &["no", "x"]);
}

#[test]
fn size_limit_reads_the_config_suffix_file() {
    let results = resolve(&[], &["size-limit"]);
    assert_entries(
        &output(&results, "size-limit").entries,
        &["size-limit.config.{js,cjs,mjs,ts,mts,cts}"],
    );
}

#[test]
fn remark_picks_the_candidate_like_load_plugin() {
    let manifest = r#"{ "name": "app", "remarkConfig": { "plugins": ["@acme/lint", "@acme/gfm", "@acme", "remark-toc", "pkg/sub", "@acme/remark-x/sub"] } }"#;
    let results = resolve(&[("package.json", manifest)], &["remark-cli", "@acme/gfm"]);
    let out = output(&results, "remark");
    assert_deps(
        out,
        &[
            "@acme/remark-lint",
            "@acme/gfm",
            "@acme/remark",
            "remark-toc",
            "pkg",
            "@acme/remark-x",
        ],
    );
    assert_absent(out, &["remark-pkg", "@acme/remark-gfm", "@acme/lint"]);
}

#[test]
fn biome_root_reference_is_the_config_of_the_farthest_ancestor() {
    let repo = std::env::temp_dir().join(format!("skif-biome-root-{}", std::process::id()));
    let app = repo.join("packages/app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(repo.join("biome.json"), r#"{ "extends": ["@org/biome-base"] }"#).unwrap();
    std::fs::write(repo.join("packages/biome.json"), "{}").unwrap();
    std::fs::write(app.join("package.json"), r#"{"name":"app"}"#).unwrap();
    std::fs::write(app.join("biome.json"), r#"{ "extends": ["//"] }"#).unwrap();
    let dependencies = vec!["@biomejs/biome".to_owned()];
    let scripts = IndexMap::new();
    let plugins = IndexMap::new();
    let fs = skif_core::OsFs;
    let ctx = PluginContext {
        dir: &app,
        is_root: false,
        package_name: Some("app"),
        dependencies: &dependencies,
        inherited_dependencies: &[],
        scripts: &scripts,
        fs: &fs,
        plugins: &plugins,
    };
    let results = resolve_plugins(&ctx);
    let out = output(&results, "biome");
    let root_config_read = out.config_files.iter().any(|f| f == &app.join("../../biome.json"));
    let dependencies_found = deps(out).contains("@org/biome-base");
    std::fs::remove_dir_all(&repo).unwrap();
    assert!(root_config_read, "{:?}", out.config_files);
    assert!(dependencies_found, "{:?}", deps(out));
}

#[test]
fn pre_commit_counts_its_own_package_as_used() {
    let results = resolve(&[], &["@fastify/pre-commit"]);
    let out = output(&results, "pre-commit");
    assert_deps(out, &["@fastify/pre-commit"]);
    assert_absent(out, &["pre-commit"]);
    let nested = resolve_with(&[], &["pre-commit"], false, &IndexMap::new());
    assert!(!nested.iter().any(|(id, _)| *id == "pre-commit"), "root only");
}

#[test]
fn parcel_reads_plugins_of_parcelrc() {
    let results = resolve(
        &[(
            ".parcelrc",
            r#"{
              "extends": ["@parcel/config-default", "./base.parcelrc"],
              "transformers": { "*.svg": ["...", "@parcel/transformer-svg-react"], "*.local": "./plugins/local.js" },
              "reporters": ["...", "parcel-reporter-static-files-copy"],
              "bundler": "@parcel/bundler-experimental"
            }"#,
        )],
        &["parcel"],
    );
    let out = output(&results, "parcel");
    assert_deps(
        out,
        &[
            "@parcel/config-default",
            "@parcel/transformer-svg-react",
            "parcel-reporter-static-files-copy",
            "@parcel/bundler-experimental",
        ],
    );
    assert_entries(&out.entries, &["plugins/local.js"]);
    assert!(!deps(out).contains("..."));
}

#[test]
fn next_intl_and_dependency_cruiser_rows() {
    let results = resolve(&[], &["next-intl", "dependency-cruiser"]);
    assert_entries(
        &output(&results, "next-intl").production_entries,
        &["{,src/}i18n/request.{js,jsx,ts,tsx}"],
    );
    let cruiser = output(&results, "dependency-cruiser");
    assert_entries(&cruiser.entries, &[".dependency-cruiser.{js,cjs,mjs,json}"]);
    assert!(cruiser.provided_binaries.contains(&"depcruise".to_owned()));
}

#[test]
fn dependency_cruiser_config_flag_names_a_config_file() {
    let results = resolve_with_scripts(
        &[("tools/cruiser.cjs", "module.exports = {};")],
        &["dependency-cruiser"],
        &[("deps", "depcruise src --config tools/cruiser.cjs")],
    );
    let out = output(&results, "dependency-cruiser");
    assert!(out.config_files.iter().any(|f| f.ends_with("tools/cruiser.cjs")));
}

#[test]
fn commitizen_adapter_from_package_json_and_czrc() {
    let manifest = (
        "package.json",
        r#"{"name":"app","config":{"commitizen":{"path":"./node_modules/cz-conventional-changelog"}}}"#,
    );
    let results = resolve(&[manifest], &["commitizen"]);
    assert_deps(output(&results, "commitizen"), &["cz-conventional-changelog"]);
    let results = resolve(&[(".czrc", r#"{"path":"@commitlint/cz-commitlint"}"#)], &["commitizen"]);
    assert_deps(output(&results, "commitizen"), &["@commitlint/cz-commitlint"]);
}

#[test]
fn electron_forge_reads_plugins_from_package_json_and_constructors() {
    let manifest = (
        "package.json",
        r#"{"name":"app","config":{"forge":{"makers":[{"name":"@electron-forge/maker-zip"}],"plugins":[{"name":"@electron-forge/plugin-webpack","config":{"mainConfig":"./webpack.main.config.js","renderer":{"config":"./webpack.renderer.config.js","entryPoints":[{"js":"./src/renderer.ts","preload":{"js":"./src/preload.ts"}}]}}}]}}}"#,
    );
    let main_config = ("webpack.main.config.js", "module.exports = { entry: './src/main.ts' };");
    let results = resolve(&[manifest, main_config], &["@electron-forge/cli"]);
    let out = output(&results, "electron-forge");
    assert_deps(out, &["@electron-forge/maker-zip", "@electron-forge/plugin-webpack"]);
    assert_entries(&out.production_entries, &["src/renderer.ts", "src/preload.ts"]);
    assert_entries(&out.entries, &["src/main.ts"]);

    let results = resolve(
        &[
            (
                "forge.config.ts",
                r#"import { VitePlugin } from "@electron-forge/plugin-vite";
                export default { plugins: [new VitePlugin({ build: [{ entry: "src/main.ts", config: "vite.main.config.ts" }], renderer: [{ name: "main_window", config: "vite.renderer.config.ts" }] })] };"#,
            ),
            ("vite.main.config.ts", "export default {};"),
        ],
        &["@electron-forge/cli", "vite"],
    );
    let out = output(&results, "electron-forge");
    assert_entries(&out.production_entries, &["src/main.ts"]);
    assert!(
        out.config_files.iter().any(|f| f.ends_with("vite.main.config.ts")),
        "{:?}",
        out.config_files
    );
}

#[test]
fn serverless_framework_handlers_plugins_and_file_variables() {
    let results = resolve(
        &[
            (
                "serverless.yml",
                "service: api\nplugins:\n  - serverless-offline\n  - ./plugins/local.js\ncustom:\n  esbuild:\n    inject: [./shims.js]\nfunctions:\n  hello:\n    handler: src/hello.main\n  typed:\n    handler: src/typed.ts.handler\n  more: ${file(./functions.yml):extra}\n",
            ),
            ("functions.yml", "extra:\n  handler: src/extra.run\n"),
        ],
        &["serverless"],
    );
    let out = output(&results, "serverless-framework");
    assert_deps(out, &["serverless-offline", "esbuild"]);
    assert_entries(
        &out.production_entries,
        &[
            "src/hello.{js,ts}",
            "src/typed.ts",
            "plugins/local.js",
            "shims.js",
            "src/extra.{js,ts}",
        ],
    );
}

#[test]
fn github_action_runs_its_sources_but_not_its_build_output() {
    let results = resolve(
        &[(
            "action.yml",
            "name: x\nruns:\n  using: node20\n  pre: src/setup.js\n  main: dist/index.js\n  post: src/cleanup.js\n",
        )],
        &["@actions/core"],
    );
    let out = output(&results, "github-action");
    assert_entries(&out.entries, &["src/setup.js", "src/cleanup.js"]);
    assert!(!out.entries.iter().any(|e| e.starts_with("dist/")), "{:?}", out.entries);
}
