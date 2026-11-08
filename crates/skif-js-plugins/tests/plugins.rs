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

const ADR_IDS: [&str; 200] = [
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
    "vue",
    "unplugin-auto-import",
    "unplugin-vue-components",
    "unplugin-vue-router",
    "mdx",
    "next-mdx",
    "starlight",
    "pino",
    "execa",
    "zx",
    "nano-spawn",
    "azure-pipelines",
    "sapper",
    "tauri",
    "capacitor",
    "convex",
    "payload",
    "wxt",
    "hardhat",
    "temporal",
    "react-email",
    "openapi-ts",
    "danger",
    "changelogen",
    "purgecss",
    "fumadocs",
    "nuxtjs-i18n",
    "playwright-ct",
    "sst",
    "qwik",
    "quasar",
    "stencil",
    "eleventy",
    "cucumber",
    "relay",
    "orval",
    "vite-plugin-pwa",
    "xo",
    "textlint",
    "rstest",
    "sveltejs-package",
    "taskfile",
    "mise",
    "moonrepo",
    "wireit",
    "travis",
    "buf",
    "nano-staged",
    "astro-db",
    "astro-og-canvas",
    "bumpp",
    "changelogithub",
    "create-typescript-app",
    "eve",
    "expressive-code",
    "kysely-codegen",
    "lockfile-lint",
    "lost-pixel",
    "node-modules-inspector",
    "vercel-og",
    "vite-pwa-assets-generator",
    "astro-markdoc",
    "borp",
    "ladle",
    "linthtml",
    "lunaria",
    "marko",
    "mcp",
    "mdxlint",
    "npm-package-json-lint",
    "osls",
    "playwright-test",
    "raycast",
    "react-cosmos",
    "tsrs",
    "varlock",
    "yorkie",
    "filesystem-routing",
    "flue",
    "laravel-vite-plugin",
    "railway",
    "solidjs-vite-plugin",
    "vite-plugin-pages",
    "vite-plugin-vue-layouts-next",
    "vite-plus",
    "openclaw",
    "typescript-content-mapper",
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
                "service: api\nplugins:\n  - serverless-offline\n  - ./plugins/local.js\ncustom:\n  esbuild:\n    inject: [./shims.js]\nfunctions:\n  hello:\n    handler: src/hello.main\n  typed:\n    handler: src/typed.ts.handler\n  more: ${file(./functions.yml):extra}\n  dynamic:\n    handler: ${self:custom.dir}/dynamic.main\n  glob:\n    handler: src/dynamic/*.main\n",
            ),
            ("functions.yml", "extra:\n  handler: src/extra.run\n"),
        ],
        &["serverless"],
    );
    let out = output(&results, "serverless-framework");
    assert!(
        !out.production_entries.iter().any(|e| e.contains("dynamic")),
        "{:?}",
        out.production_entries
    );
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

#[test]
fn vue_reads_configure_webpack_and_the_lint_script() {
    let config = (
        "vue.config.js",
        r#"module.exports = { configureWebpack: { entry: { admin: "./src/admin.ts" }, module: { rules: [{ test: /\.md$/, loader: "markdown-loader" }] } } };"#,
    );
    let results = resolve_with_scripts(&[config], &["vue"], &[("lint", "vue-cli-service lint --fix")]);
    let out = output(&results, "vue");
    assert_entries(&out.entries, &["src/admin.ts"]);
    assert_deps(out, &["markdown-loader", "@vue/cli-plugin-eslint"]);
    let results = resolve_with_scripts(&[], &["vue"], &[("build", "vue-cli-service build && eslint lint")]);
    assert_absent(output(&results, "vue"), &["@vue/cli-plugin-eslint"]);
    let results = resolve_with_scripts(&[], &["vue"], &[("build", "vue-cli-service build --skip lint")]);
    assert_absent(output(&results, "vue"), &["@vue/cli-plugin-eslint"]);
    let results = resolve_with_scripts(&[], &["vue"], &[("lint", "vue-cli-service --fix lint")]);
    assert_deps(output(&results, "vue"), &["@vue/cli-plugin-eslint"]);
}

#[test]
fn unplugin_generated_declarations_are_entries() {
    let files = [
        (
            "components.d.ts",
            "/* eslint-disable */\n// Generated by unplugin-vue-components\nexport {}\ndeclare module 'vue' { export interface GlobalComponents { Card: typeof import('./src/components/Card.vue')['default'] } }",
        ),
        (
            "src/auto-imports.d.ts",
            "// Generated by unplugin-auto-import\nexport {}\ndeclare global { const ref: typeof import('vue')['ref'] }",
        ),
        ("src/env.d.ts", "/// <reference types=\"vite/client\" />"),
    ];
    let results = resolve(&files, &["unplugin-vue-components", "unplugin-auto-import"]);
    assert_eq!(output(&results, "unplugin-vue-components").entries, ["components.d.ts"]);
    assert_eq!(
        output(&results, "unplugin-auto-import").entries,
        ["src/auto-imports.d.ts"]
    );
}

#[test]
fn unplugin_vue_router_routes_need_the_plugin_or_its_generated_types() {
    let results = resolve(&[], &["unplugin-vue-router"]);
    assert_entries(
        &output(&results, "unplugin-vue-router").production_entries,
        &["src/pages/**/*.{vue,md}"],
    );
    let results = resolve(&[], &["vue-router"]);
    assert_eq!(
        output(&results, "unplugin-vue-router").production_entries,
        Vec::<String>::new()
    );
    let generated = (
        "typed-router.d.ts",
        "/* eslint-disable */\n// Generated by vue-router. !! DO NOT MODIFY !!\nexport {}",
    );
    let results = resolve(&[generated], &["vue-router"]);
    assert_entries(
        &output(&results, "unplugin-vue-router").production_entries,
        &["src/pages/**/*.{vue,md}"],
    );
}

#[test]
fn mdx_reads_the_language_server_plugins_of_tsconfig() {
    let results = resolve(
        &[(
            "tsconfig.json",
            r#"{"compilerOptions":{},"mdx":{"plugins":["remark-frontmatter",["remark-gfm",{}]]}}"#,
        )],
        &["astro"],
    );
    assert_deps(output(&results, "mdx"), &["remark-frontmatter", "remark-gfm"]);
    assert!(!resolve(&[], &["react"]).iter().any(|(id, _)| *id == "mdx"));
}

#[test]
fn next_mdx_reads_plugins_from_the_create_mdx_call() {
    let results = resolve(
        &[(
            "next.config.mjs",
            r#"import createMDX from "@next/mdx";
            import remarkMath from "remark-math";
            const withMDX = createMDX({ options: { remarkPlugins: ["remark-gfm", remarkMath], rehypePlugins: [["rehype-katex", { strict: true }]] } });
            export default withMDX({ pageExtensions: ["mdx", "tsx"] });"#,
        )],
        &["@next/mdx", "next"],
    );
    let out = output(&results, "next-mdx");
    assert_deps(out, &["remark-gfm", "rehype-katex"]);
    assert_entries(&out.production_entries, &["{,src/}mdx-components.{js,jsx,ts,tsx}"]);
    assert!(!out.entries_incomplete);
}

#[test]
fn starlight_component_overrides_and_custom_css_are_entries() {
    let results = resolve(
        &[(
            "astro.config.mjs",
            r#"import { defineConfig } from "astro/config";
            import starlight from "@astrojs/starlight";
            export default defineConfig({ integrations: [starlight({ title: "Docs", components: { Head: "./src/components/Head.astro" }, customCss: ["./src/styles/custom.css", "@fontsource/inter"] })] });"#,
        )],
        &["astro", "@astrojs/starlight"],
    );
    let out = output(&results, "starlight");
    assert_entries(
        &out.production_entries,
        &["src/components/Head.astro", "src/styles/custom.css"],
    );
    assert_deps(out, &["@fontsource/inter"]);
}

#[test]
fn pino_transport_targets_are_dependencies_or_entries() {
    let results = resolve(
        &[(
            "src/log.ts",
            r#"import pino, { transport as makeTransport } from "pino";
            export const logger = pino({ transport: { target: "pino-pretty" } });
            export const multi = makeTransport({ targets: [{ target: "./transports/file.mjs" }, { target: "@axiomhq/pino" }] });
            export const piped = pino.transport({ pipeline: [{ target: "pino-syslog" }] });"#,
        )],
        &["pino"],
    );
    let out = output(&results, "pino");
    assert_deps(out, &["pino-pretty", "@axiomhq/pino", "pino-syslog"]);
    assert_entries(&out.entries, &["src/transports/file.mjs"]);
}

#[test]
fn execa_zx_and_nano_spawn_commands_are_scripts() {
    let files = [
        (
            "scripts/release.ts",
            r#"import { execa, $ } from "execa";
            await execa("changeset", ["publish", someFlag]);
            await $`tsx scripts/notes.ts ${version}`;"#,
        ),
        ("scripts/deploy.mjs", "#!/usr/bin/env zx\nawait $`wrangler deploy`;\n"),
        (
            "scripts/check.js",
            r#"import spawn from "nano-spawn"; await spawn("eslint", ["."]);"#,
        ),
    ];
    let results = resolve(&files, &["execa", "zx", "nano-spawn"]);
    let binaries = |id: &str| output(&results, id).referenced_binaries.clone();
    assert!(
        binaries("execa").contains(&"changeset".to_owned()),
        "{:?}",
        binaries("execa")
    );
    assert!(binaries("execa").contains(&"tsx".to_owned()));
    assert_entries(&output(&results, "execa").entries, &["scripts/notes.ts"]);
    assert!(binaries("zx").contains(&"wrangler".to_owned()), "{:?}", binaries("zx"));
    assert!(
        binaries("nano-spawn").contains(&"eslint".to_owned()),
        "{:?}",
        binaries("nano-spawn")
    );
}

#[test]
fn execa_default_import_and_zx_globals_are_read() {
    let files = [
        (
            "scripts/legacy.js",
            r#"import execa from "execa";
            await execa("prettier", ["--write", "."]);
            await execa.command("tsc --build");
            execa.sync("vitest", ["run"]);"#,
        ),
        (
            "scripts/globals.mjs",
            "import 'zx/globals';\nawait $`wrangler deploy`;\n",
        ),
        ("scripts/npx.mjs", "#!/usr/bin/env -S npx zx\nawait $`turbo build`;\n"),
    ];
    let results = resolve(&files, &["execa", "zx"]);
    let binaries = |id: &str| output(&results, id).referenced_binaries.clone();
    for binary in ["prettier", "tsc", "vitest"] {
        assert!(
            binaries("execa").contains(&binary.to_owned()),
            "{:?}",
            binaries("execa")
        );
    }
    for binary in ["wrangler", "turbo"] {
        assert!(binaries("zx").contains(&binary.to_owned()), "{:?}", binaries("zx"));
    }
}

#[test]
fn execa_and_nano_spawn_skip_calls_with_an_unsafe_argument() {
    let files = [
        (
            "scripts/release.ts",
            r#"import { execa } from "execa";
            await execa("node", ["scripts/build.js", "$(rm -rf /)"]);"#,
        ),
        (
            "scripts/check.js",
            r#"import spawn from "nano-spawn"; await spawn("eslint", ["src", "a && b"]);"#,
        ),
    ];
    let results = resolve(&files, &["execa", "nano-spawn"]);
    for id in ["execa", "nano-spawn"] {
        let out = output(&results, id);
        assert!(
            out.referenced_binaries.is_empty(),
            "{id}: {:?}",
            out.referenced_binaries
        );
        assert!(out.entries.is_empty(), "{id}: {:?}", out.entries);
    }
}

#[test]
fn azure_pipelines_script_steps_and_templates() {
    let results = resolve(
        &[
            (
                "azure-pipelines.yml",
                "stages:\n- stage: build\n  jobs:\n  - job: web\n    steps:\n    - script: pnpm exec vitest run\n    - bash: node scripts/check.mjs\n      workingDirectory: packages/web\n    - script: echo $(Build.SourcesDirectory)\n      workingDirectory: $(Build.SourcesDirectory)/x\n    - template: templates/lint.yml\n",
            ),
            ("templates/lint.yml", "steps:\n- script: eslint .\n"),
        ],
        &[],
    );
    let out = output(&results, "azure-pipelines");
    assert!(
        out.referenced_binaries.contains(&"vitest".to_owned()),
        "{:?}",
        out.referenced_binaries
    );
    assert!(
        out.referenced_binaries.contains(&"eslint".to_owned()),
        "{:?}",
        out.referenced_binaries
    );
    assert_entries(&out.entries, &["packages/web/scripts/check.mjs"]);
    let nested = resolve_with(&[("azure-pipelines.yml", "steps: []")], &[], false, &IndexMap::new());
    assert!(!nested.iter().any(|(id, _)| *id == "azure-pipelines"), "root only");
}

#[test]
fn sapper_declares_its_fixed_layout() {
    let results = resolve(&[], &["sapper"]);
    assert_entries(
        &output(&results, "sapper").production_entries,
        &[
            "src/{client,server,service-worker}.{js,ts}",
            "src/routes/**/*.{svelte,html,js,ts}",
        ],
    );
}

#[test]
fn tauri_runs_its_before_commands_where_they_say() {
    let results = resolve(
        &[(
            "src-tauri/tauri.conf.json",
            r#"{"build":{"beforeDevCommand":"vite dev","beforeBuildCommand":{"script":"node scripts/build.js","cwd":"ui"}}}"#,
        )],
        &["@tauri-apps/cli"],
    );
    let out = output(&results, "tauri");
    assert!(
        out.referenced_binaries.contains(&"vite".to_owned()),
        "{:?}",
        out.referenced_binaries
    );
    assert_entries(&out.entries, &["ui/scripts/build.js"]);
    let results = resolve(
        &[(
            "src-tauri/Tauri.toml",
            "[build]\nbefore-bundle-command = \"node bundle.js\"\n",
        )],
        &["@tauri-apps/cli"],
    );
    assert_entries(&output(&results, "tauri").entries, &["bundle.js"]);
}

#[test]
fn capacitor_credits_plugins_and_native_platforms() {
    let results = resolve(
        &[
            (
                "capacitor.config.json",
                r#"{"appId":"x","includePlugins":["@capacitor/camera"]}"#,
            ),
            ("android/capacitor.settings.gradle", "include ':capacitor-android'"),
            ("ios/App/Podfile", "platform :ios"),
        ],
        &["@capacitor/core"],
    );
    let out = output(&results, "capacitor");
    assert_deps(out, &["@capacitor/camera", "@capacitor/android", "@capacitor/ios"]);
}

#[test]
fn convex_functions_directory_from_convex_json() {
    let results = resolve(&[], &["convex"]);
    assert_entries(&output(&results, "convex").production_entries, &["convex/**/*.{js,ts}"]);
    let results = resolve(&[("convex.json", r#"{"functions":"src/convex/"}"#)], &["convex"]);
    assert_entries(
        &output(&results, "convex").production_entries,
        &["src/convex/**/*.{js,ts}"],
    );
}

#[test]
fn payload_import_map_follows_the_admin_route() {
    let config = r#"import { buildConfig } from "payload";
        export default buildConfig({ routes: { admin: "/cms" }, collections: [] });"#;
    let results = resolve(&[("src/payload.config.ts", config)], &["payload"]);
    let out = output(&results, "payload");
    assert_entries(
        &out.entries,
        &["app/(payload)/cms/importMap.js", "src/app/(payload)/cms/importMap.js"],
    );
    assert!(out.ignore.contains(&"src/payload-types.ts".to_owned()));
    assert!(!out.entries_incomplete);
    let config = r#"import { buildConfig } from "payload";
        export default buildConfig({ admin: { importMap: { importMapFile: "./app/admin/importMap.js" } } });"#;
    let results = resolve(&[("payload.config.ts", config)], &["payload"]);
    assert_entries(&output(&results, "payload").entries, &["app/admin/importMap.js"]);
}

#[test]
fn wxt_entrypoints_and_modules() {
    let results = resolve(
        &[(
            "wxt.config.ts",
            r#"import { defineConfig } from "wxt"; export default defineConfig({ modules: ["@wxt-dev/module-react"] });"#,
        )],
        &["wxt"],
    );
    let out = output(&results, "wxt");
    assert_entries(&out.production_entries, &["entrypoints/**/*"]);
    assert_deps(out, &["@wxt-dev/module-react"]);
}

#[test]
fn declarative_rows_of_issue_40() {
    let results = resolve(
        &[],
        &[
            "hardhat",
            "@temporalio/worker",
            "@hey-api/openapi-ts",
            "danger",
            "changelogen",
            "@fullhuman/postcss-purgecss",
            "fumadocs-mdx",
            "@nuxtjs/i18n",
        ],
    );
    let hardhat = output(&results, "hardhat");
    assert_entries(&hardhat.entries, &["hardhat.config.{js,cjs,mjs,ts,cts,mts}"]);
    assert_deps(hardhat, &["hardhat"]);
    assert_entries(
        &output(&results, "temporal").production_entries,
        &["src/workflows{,/index}.{js,cjs,mjs,ts,cts,mts}"],
    );
    assert_entries(
        &output(&results, "openapi-ts").entries,
        &["openapi-ts.config.{js,ts,mjs,cjs,mts,cts}"],
    );
    assert_entries(&output(&results, "danger").entries, &["dangerfile.{js,cjs,mjs,ts}"]);
    assert_entries(
        &output(&results, "changelogen").entries,
        &["changelog.config.{js,ts,mjs,cjs,mts,cts}"],
    );
    assert_entries(&output(&results, "purgecss").entries, &["purgecss.config.js"]);
    assert_entries(&output(&results, "fumadocs").entries, &["content/**/*.mdx"]);
    assert_entries(
        &output(&results, "nuxtjs-i18n").entries,
        &["i18n/i18n.config.{js,mjs,ts}"],
    );
    let nested = resolve_with(&[], &["changelogen"], false, &IndexMap::new());
    assert!(!nested.iter().any(|(id, _)| *id == "changelogen"), "root only");
}

#[test]
fn react_email_preview_commands_and_dir() {
    let results = resolve_with_scripts(&[], &["react-email"], &[("dev", "email dev --dir src/mail")]);
    let out = output(&results, "react-email");
    assert_deps(out, &["@react-email/ui", "@react-email/preview-server"]);
    assert!(out.referenced_dependencies.iter().all(|d| d.optional));
    assert_entries(&out.entries, &["emails/**/*.tsx", "src/mail/**/*.tsx"]);
    let results = resolve_with_scripts(&[], &["react-email"], &[("export", "email export")]);
    assert_absent(output(&results, "react-email"), &["@react-email/ui"]);
}

#[test]
fn playwright_ct_reads_its_config_like_playwright() {
    let results = resolve(
        &[(
            "playwright-ct.config.ts",
            r#"export default { testDir: "./tests/ct", reporter: [["playwright-ctrf-json-reporter"]] };"#,
        )],
        &["@playwright/experimental-ct-react"],
    );
    let out = output(&results, "playwright-ct");
    assert_entries(
        &out.entries,
        &[
            "tests/ct/**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "playwright/index.{js,ts,jsx,tsx}",
        ],
    );
    assert_deps(out, &["playwright-ctrf-json-reporter"]);
}

#[test]
fn sst_handlers_in_the_config_and_the_modules_it_imports() {
    let results = resolve(
        &[
            (
                "sst.config.ts",
                r#"/// <reference path="./.sst/platform/config.d.ts" />
                export default $config({
                  app() { return { name: "app", home: "aws" }; },
                  async run() {
                    new sst.aws.Function("Cron", { handler: "packages/functions/src/cron.handler" });
                    const infra = await import("./infra/api");
                    await import("./infra");
                    await import("./infra/jobs.js");
                    return { api: infra.api.url };
                  },
                });"#,
            ),
            (
                "infra/api.ts",
                r#"import { bucket } from "./storage";
                export const api = new sst.aws.ApiGatewayV2("Api");
                api.route("GET /", "packages/functions/src/api.handler");
                api.route("POST /", { handler: "packages/functions/src/post.main" });"#,
            ),
            (
                "infra/storage.ts",
                r#"export const bucket = new sst.aws.Bucket("B", { notifications: [{ name: "n", function: { handler: "packages/functions/src/notify.handler" } }] });"#,
            ),
            (
                "infra/index.ts",
                r#"new sst.aws.Function("Edge", { handler: "edge/index.ts.handler" });"#,
            ),
            (
                "infra/jobs.ts",
                r#"import "./queue.mjs"; import "./api.js"; new sst.aws.Cron("Jobs", { function: { handler: "jobs/run.handler" } });"#,
            ),
            (
                "infra/queue.mts",
                r#"new sst.aws.Queue("Q").subscribe({ handler: "jobs/queue.handler" });"#,
            ),
        ],
        &["sst"],
    );
    let out = output(&results, "sst");
    let any = "{ts,tsx,mts,cts,js,jsx,mjs,cjs}";
    assert_entries(
        &out.production_entries,
        &[
            &format!("packages/functions/src/cron.{any}"),
            &format!("packages/functions/src/api.{any}"),
            &format!("packages/functions/src/post.{any}"),
            &format!("packages/functions/src/notify.{any}"),
            "edge/index.ts",
            &format!("jobs/run.{any}"),
            &format!("jobs/queue.{any}"),
        ],
    );
    assert!(!out.entries_incomplete);
}

#[test]
fn qwik_entries_follow_src_dir_and_routes_dir() {
    let results = resolve(
        &[(
            "vite.config.ts",
            r#"import { qwikVite } from "@builder.io/qwik/optimizer";
            import { qwikCity } from "@builder.io/qwik-city/vite";
            export default defineConfig(() => ({ plugins: [qwikCity({ routesDir: "app/pages" }), qwikVite({ srcDir: "app" })] }));"#,
        )],
        &["@builder.io/qwik", "vite"],
    );
    let out = output(&results, "qwik");
    assert_entries(&out.entries, &["app/entry.dev.tsx"]);
    assert_entries(
        &out.production_entries,
        &["app/root.tsx", "app/entry.*.tsx", "app/pages/**/*.{tsx,ts,md,mdx}"],
    );
    assert!(out.virtual_modules.contains(&"@qwik-city-plan".to_owned()));
}

#[test]
fn quasar_source_files_boot_and_css() {
    let results = resolve(
        &[(
            "quasar.config.js",
            r#"const { configure } = require("quasar/wrappers");
            module.exports = configure(function (ctx) {
              return {
                boot: ["i18n", { path: "axios.ts", server: false }],
                css: ["app.scss", "~animate.css/animate.min.css"],
                sourceFiles: { router: "src/routing/main" },
                build: { vitePlugins: [] },
              };
            });"#,
        )],
        &["@quasar/app-vite"],
    );
    let out = output(&results, "quasar");
    assert_entries(
        &out.production_entries,
        &[
            "src/boot/i18n.{js,ts}",
            "src/boot/axios.ts",
            "src/css/app.scss",
            "src/routing/main.{js,ts}",
            "src/App.vue",
            "src-ssr/server.{js,ts}",
        ],
    );
    assert_deps(out, &["animate.css"]);
}

#[test]
fn stencil_global_files_tsconfig_and_setup() {
    let results = resolve(
        &[
            (
                "stencil.config.ts",
                r#"import { Config } from "@stencil/core";
                export const config: Config = {
                  srcDir: "lib",
                  globalScript: "lib/global/app.ts",
                  globalStyle: "lib/global/app.css",
                  tsconfig: "tsconfig.lib.json",
                  outputTargets: [{ type: "www" }, { type: "global-style", input: "lib/theme.css" }],
                  testing: { setupFilesAfterEnv: ["./jest.setup.ts"] },
                };"#,
            ),
            ("tsconfig.lib.json", r#"{"compilerOptions":{"types":["jest"]}}"#),
        ],
        &["@stencil/core", "typescript"],
    );
    let out = output(&results, "stencil");
    assert_entries(
        &out.production_entries,
        &[
            "lib/**/*.tsx",
            "lib/global/app.ts",
            "lib/global/app.css",
            "lib/theme.css",
        ],
    );
    assert_entries(&out.entries, &["jest.setup.ts", "**/*.spec.{ts,tsx}"]);
    assert!(out.config_files.iter().any(|f| f.ends_with("tsconfig.lib.json")));
}

#[test]
fn eleventy_input_and_data_directories() {
    let results = resolve(
        &[(
            "eleventy.config.js",
            r#"module.exports = function (eleventyConfig) {
              eleventyConfig.addPassthroughCopy("img");
              return { dir: { input: "site", data: "data" }, templateFormats: ["md", "11ty.js"] };
            };"#,
        )],
        &["@11ty/eleventy"],
    );
    let out = output(&results, "eleventy");
    assert_entries(
        &out.production_entries,
        &[
            "site/data/**/*.{js,cjs,mjs}",
            "site/**/*.{md,11ty.js}",
            "site/**/*.11tydata.{js,cjs,mjs}",
        ],
    );
}

#[test]
fn cucumber_profiles_imports_and_formatters() {
    let results = resolve(
        &[(
            "cucumber.yml",
            "default:\n  import: [steps/**/*.ts]\n  format: [progress, '@cucumber/pretty-formatter', 'html:out.html']\nci:\n  require: [support/env.js]\n  format: [['./reporters/custom.js', 'out.txt']]\n",
        )],
        &["@cucumber/cucumber"],
    );
    let out = output(&results, "cucumber");
    assert_entries(
        &out.entries,
        &[
            "steps/**/*.ts",
            "support/env.js",
            "features/**/*.{js,cjs,mjs}",
            "reporters/custom.js",
        ],
    );
    assert_deps(out, &["@cucumber/pretty-formatter"]);
    assert_absent(out, &["html", "progress"]);
}

#[test]
fn relay_artifact_directories() {
    let manifest = (
        "package.json",
        r#"{"name":"app","relay":{"src":"./src","artifactDirectory":"./src/__gen__"}}"#,
    );
    let results = resolve(&[manifest], &["babel-plugin-relay"]);
    assert_entries(&output(&results, "relay").production_entries, &["src/__gen__/**"]);
    let results = resolve(
        &[("relay.config.json", r#"{"projects":{"web":{"language":"typescript"}}}"#)],
        &["vite-plugin-relay"],
    );
    assert_entries(&output(&results, "relay").production_entries, &["**/__generated__/*"]);
}

#[test]
fn orval_mutators_and_transformers() {
    let results = resolve(
        &[(
            "orval.config.ts",
            r#"import { defineConfig } from "orval";
            export default defineConfig({ api: { input: { target: "./spec.yaml", override: { transformer: "./transform.js" } }, output: { override: { mutator: { path: "./src/http.ts", name: "http" } } } } });"#,
        )],
        &["orval"],
    );
    assert_entries(&output(&results, "orval").entries, &["transform.js", "src/http.ts"]);
}

#[test]
fn vite_plugin_pwa_inject_manifest_worker() {
    let results = resolve(
        &[(
            "vite.config.ts",
            r#"import { VitePWA } from "vite-plugin-pwa";
            export default { plugins: [VitePWA({ strategies: "injectManifest", srcDir: "src", filename: "sw.ts" })] };"#,
        )],
        &["vite-plugin-pwa"],
    );
    assert_entries(&output(&results, "vite-plugin-pwa").production_entries, &["src/sw.ts"]);
    let results = resolve(
        &[(
            "nuxt.config.ts",
            r#"export default defineNuxtConfig({ modules: ["@vite-pwa/nuxt"], pwa: { strategies: "injectManifest" } });"#,
        )],
        &["@vite-pwa/nuxt"],
    );
    assert_entries(
        &output(&results, "vite-plugin-pwa").production_entries,
        &["public/sw.js"],
    );
}

#[test]
fn xo_legacy_config_names_eslint_packages() {
    let manifest = (
        "package.json",
        r#"{"name":"app","xo":{"extends":"xo-react","plugins":["unicorn"],"parser":"@babel/eslint-parser"}}"#,
    );
    let results = resolve(&[manifest], &["xo"]);
    let out = output(&results, "xo");
    assert_deps(
        out,
        &[
            "eslint-config-xo-react",
            "eslint-plugin-unicorn",
            "@babel/eslint-parser",
        ],
    );
    assert_entries(&out.entries, &["xo.config.{js,cjs,mjs,ts,cts,mts}"]);
}

#[test]
fn textlint_rules_presets_filters_and_plugins() {
    let results = resolve_with_scripts(
        &[(
            ".textlintrc.json",
            r#"{"rules":{"no-todo":true,"off-rule":false,"preset-ja-technical-writing":true,"@textlint-rule/preset-google":{},"./rules/local.js":true},"filters":{"comments":true},"plugins":["@textlint/markdown"]}"#,
        )],
        &["textlint", "textlint-rule-preset-ja-technical-writing"],
        &[("lint", "textlint --rule terminology --preset jtf-style docs")],
    );
    let out = output(&results, "textlint");
    assert_deps(
        out,
        &[
            "textlint-rule-no-todo",
            "textlint-rule-preset-ja-technical-writing",
            "@textlint-rule/textlint-rule-preset-google",
            "textlint-filter-rule-comments",
            "@textlint/textlint-plugin-markdown",
            "textlint-rule-terminology",
            "textlint-rule-preset-jtf-style",
        ],
    );
    assert_absent(out, &["textlint-rule-off-rule"]);
    assert_entries(&out.entries, &["rules/local.js"]);
}

#[test]
fn rstest_include_environment_setup_and_projects() {
    let results = resolve(
        &[
            (
                "rstest.config.ts",
                r#"import { defineConfig } from "@rstest/core";
                export default defineConfig({ projects: ["packages/*", { root: "web", include: ["tests/**/*.test.tsx"], testEnvironment: "happy-dom", setupFiles: ["./setup.ts"], browser: { enabled: true } }], coverage: { provider: "v8" } });"#,
            ),
            (
                "packages/a/rstest.config.ts",
                r#"export default { testEnvironment: "jsdom" };"#,
            ),
        ],
        &["@rstest/core"],
    );
    let out = output(&results, "rstest");
    assert_entries(&out.entries, &["web/tests/**/*.test.tsx", "web/setup.ts"]);
    assert_deps(
        out,
        &[
            "happy-dom",
            "@rstest/browser",
            "playwright",
            "jsdom",
            "@rstest/coverage-v8",
        ],
    );
    assert!(
        out.config_files
            .iter()
            .any(|f| f.ends_with("packages/a/rstest.config.ts"))
    );
}

#[test]
fn sveltejs_package_maps_output_to_library_sources() {
    let results = resolve_with_scripts(
        &[],
        &["@sveltejs/package"],
        &[(
            "package",
            "svelte-kit sync && svelte-package -i src/components -o package",
        )],
    );
    let out = output(&results, "sveltejs-package");
    assert_eq!(
        out.build_dirs,
        [skif_js_plugins::BuildDir {
            out_dir: "package".to_owned(),
            root_dir: "src/components".to_owned()
        }]
    );
    let results = resolve(&[], &["@sveltejs/package"]);
    assert_eq!(output(&results, "sveltejs-package").build_dirs[0].out_dir, "dist");
}

#[test]
fn taskfile_tasks_and_includes() {
    let results = resolve(
        &[
            (
                "Taskfile.yml",
                "version: '3'\nincludes:\n  docs: ./docs/Taskfile.yml\n  other:\n    taskfile: ./other.yml\ntasks:\n  build: tsc -b\n  release: '{{.TOOL}} publish'\n  fmt:\n    cmds:\n      - 'node {{.SCRIPT}}'\n  lint:\n    cmds:\n      - eslint .\n      - cmd: node scripts/check.js\n        defer: { cmd: node scripts/cleanup.js }\n",
            ),
            ("docs/Taskfile.yml", "version: '3'\ntasks:\n  docs:\n    cmd: typedoc\n"),
        ],
        &[],
    );
    let out = output(&results, "taskfile");
    for binary in ["tsc", "eslint", "typedoc"] {
        assert!(
            out.referenced_binaries.contains(&binary.to_owned()),
            "{binary}: {:?}",
            out.referenced_binaries
        );
    }
    assert_entries(&out.entries, &["scripts/check.js", "scripts/cleanup.js"]);
    assert!(
        !out.referenced_binaries.iter().any(|binary| binary.contains("{{")),
        "{:?}",
        out.referenced_binaries
    );
    assert!(
        !out.entries.iter().any(|entry| entry.contains("{{")),
        "{:?}",
        out.entries
    );
    let results = resolve(&[("taskfile.dist.yaml", "version: '3'\n")], &[]);
    output(&results, "taskfile");
    let nested = resolve_with(&[("Taskfile.yml", "version: '3'\n")], &[], false, &IndexMap::new());
    assert!(!nested.iter().any(|(id, _)| *id == "taskfile"), "root only");
}

#[test]
fn mise_tasks_run_in_their_directory() {
    let results = resolve(
        &[
            (
                "mise.toml",
                "[tasks.build]\nrun = \"node scripts/build.js\"\n[tasks.web]\nrun = [\"vite build\", \"node tools/post.js\"]\ndir = \"{{config_root}}/web\"\n[tasks.tpl]\nrun = \"node {{arg(name='x')}}.js\"\n",
            ),
            (
                ".config/mise/conf.d/extra.toml",
                "[tasks.extra]\nrun = \"node extra.js\"\n",
            ),
        ],
        &[],
    );
    let out = output(&results, "mise");
    assert_entries(&out.entries, &["scripts/build.js", "web/tools/post.js", "extra.js"]);
    assert!(
        !out.referenced_binaries.contains(&"vite".to_owned()),
        "a command in another directory"
    );
}

#[test]
fn moonrepo_task_commands() {
    let results = resolve(
        &[(
            ".moon/tasks.yml",
            "tasks:\n  lint:\n    command: eslint .\n  gen:\n    command: [node, $workspaceRoot/scripts/gen.js]\n",
        )],
        &["@moonrepo/cli"],
    );
    let out = output(&results, "moonrepo");
    assert!(out.referenced_binaries.contains(&"eslint".to_owned()));
    assert_entries(&out.entries, &["scripts/gen.js"]);
}

#[test]
fn wireit_commands_from_package_json() {
    let manifest = (
        "package.json",
        r#"{"name":"app","scripts":{"build":"wireit"},"wireit":{"build":{"command":"tsc && node scripts/copy.js","dependencies":["gen"]},"gen":{"dependencies":["build"]}}}"#,
    );
    let results = resolve(&[manifest], &["wireit"]);
    let out = output(&results, "wireit");
    assert!(out.referenced_binaries.contains(&"tsc".to_owned()));
    assert_entries(&out.entries, &["scripts/copy.js"]);
}

#[test]
fn travis_before_commands() {
    let results = resolve(
        &[(
            ".travis.yml",
            "language: node_js\nbefore_install: npm i -g codecov\nbefore_script:\n  - node scripts/prepare.js\nscript: npm test\n",
        )],
        &[],
    );
    assert_entries(&output(&results, "travis").entries, &["scripts/prepare.js"]);
}

#[test]
fn buf_local_plugins_are_commands() {
    let results = resolve_with_scripts(
        &[(
            "proto/buf.gen.yaml",
            "version: v2\nplugins:\n  - local: protoc-gen-es\n    out: src/gen\n  - local: [node, tools/protoc-gen-custom.js]\n    out: x\n  - remote: buf.build/bufbuild/es\n    out: y\n",
        )],
        &["@bufbuild/buf", "@bufbuild/protoc-gen-es"],
        &[("gen", "buf generate --template proto/buf.gen.yaml")],
    );
    let out = output(&results, "buf");
    assert!(
        out.referenced_binaries.contains(&"protoc-gen-es".to_owned()),
        "{:?}",
        out.referenced_binaries
    );
    assert_entries(&out.entries, &["tools/protoc-gen-custom.js"]);
    let results = resolve(
        &[("buf.gen.yaml", "version: v1\nplugins:\n  - plugin: es\n    out: gen\n")],
        &["@bufbuild/buf"],
    );
    assert!(
        output(&results, "buf")
            .referenced_binaries
            .contains(&"protoc-gen-es".to_owned())
    );
}

#[test]
fn nano_staged_commands() {
    let manifest = (
        "package.json",
        r#"{"name":"app","nano-staged":{"*.{js,ts}":["prettier --write","eslint --fix"],"*.md":"markdownlint"}}"#,
    );
    let results = resolve(&[manifest], &["nano-staged"]);
    let out = output(&results, "nano-staged");
    for binary in ["prettier", "eslint", "markdownlint"] {
        assert!(out.referenced_binaries.contains(&binary.to_owned()), "{binary}");
    }
}

#[test]
fn declarative_plugins_of_issue_41() {
    // (id, enabling dependency, an entry, a production entry, a provided binary)
    let table = [
        ("astro-db", "@astrojs/db", Some("db/seed.{js,ts}"), None, None),
        ("astro-og-canvas", "astro-og-canvas", None, None, None),
        (
            "bumpp",
            "bumpp",
            Some("bump.config.{js,ts,mjs,cjs,mts,cts}"),
            None,
            Some("bumpp"),
        ),
        (
            "changelogithub",
            "changelogithub",
            Some(".changelogithubrc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}"),
            None,
            Some("changelogithub"),
        ),
        (
            "create-typescript-app",
            "create-typescript-app",
            Some("create-typescript-app.config.{js,cjs,mjs,ts}"),
            None,
            None,
        ),
        ("eve", "eve", Some("{evals,agents/*/evals}/**/*.eval.ts"), None, None),
        (
            "expressive-code",
            "astro-expressive-code",
            Some("ec.config.{js,mjs,cjs,ts}"),
            None,
            None,
        ),
        (
            "kysely-codegen",
            "kysely-codegen",
            Some("kysely-codegen.config.{js,ts,cjs}"),
            None,
            Some("kysely-codegen"),
        ),
        (
            "lockfile-lint",
            "lockfile-lint",
            Some(".lockfile-lintrc.{json,yaml,yml,js,ts,cjs,mjs,toml}"),
            None,
            Some("lockfile-lint"),
        ),
        (
            "lost-pixel",
            "lost-pixel",
            Some("lostpixel.config.{js,ts}"),
            None,
            Some("lost-pixel"),
        ),
        (
            "node-modules-inspector",
            "node-modules-inspector",
            None,
            None,
            Some("node-modules-inspector"),
        ),
        (
            "vercel-og",
            "@vercel/og",
            None,
            Some("{src/,}app/api/og/route.{jsx,tsx}"),
            None,
        ),
        (
            "vite-pwa-assets-generator",
            "@vite-pwa/assets-generator",
            Some("pwa-assets.config.{js,cjs,mjs,ts,cts,mts}"),
            None,
            Some("pwa-assets-generator"),
        ),
    ];
    let without = resolve(&[], &[]);
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
            assert!(out.provided_binaries.contains(&binary.to_owned()), "{id}: {out:?}");
        }
        assert!(
            !without.iter().any(|(plugin, _)| *plugin == id),
            "{id} must not be enabled without {dependency}"
        );
    }
    let nested = resolve_with(&[], &["changelogithub"], false, &IndexMap::new());
    assert!(!nested.iter().any(|(id, _)| *id == "changelogithub"), "root only");
}

#[test]
fn astro_og_canvas_loads_canvaskit_optionally() {
    let results = resolve(&[], &["astro-og-canvas"]);
    let out = output(&results, "astro-og-canvas");
    assert!(
        out.referenced_dependencies
            .iter()
            .any(|d| d.name == "canvaskit-wasm" && d.optional),
        "{out:?}"
    );
}

#[test]
fn eve_agent_files_are_production_entries() {
    let results = resolve(&[], &["eve"]);
    let out = output(&results, "eve");
    assert_entries(
        &out.production_entries,
        &[
            "{,agent/,agents/*/,agents/*/agent/}{channels,connections,hooks,skills,tools,schedules}/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
            "{,agent/,agents/*/,agents/*/agent/}extensions/*/{extension,instructions}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        ],
    );
    assert!(out.production_entries.iter().all(|pattern| !pattern.contains("+(")));
}

#[test]
fn kysely_codegen_reads_a_config_passed_with_config_file() {
    let results = resolve_with_scripts(
        &[("db/codegen.ts", "export default {}")],
        &["kysely-codegen"],
        &[("gen", "kysely-codegen --config-file db/codegen.ts")],
    );
    let out = output(&results, "kysely-codegen");
    assert!(
        out.config_files.iter().any(|path| path.ends_with("db/codegen.ts")),
        "{out:?}"
    );
}

#[test]
fn node_modules_inspector_config_is_a_config_file() {
    let results = resolve(
        &[("node-modules-inspector.config.ts", "export default {}")],
        &["node-modules-inspector"],
    );
    let out = output(&results, "node-modules-inspector");
    assert_eq!(out.config_files.len(), 1, "{out:?}");
}

#[test]
fn astro_markdoc_render_components() {
    let results = resolve(
        &[(
            "markdoc.config.mjs",
            r#"import { defineMarkdocConfig, component } from "@astrojs/markdoc/config";
export default defineMarkdocConfig({
  tags: {
    aside: { render: component("./src/components/Aside.astro"), attributes: {} },
    table: { render: component("@astrojs/markdoc/components", "Table") },
  },
  nodes: { heading: { render: "./src/components/Heading.astro#Heading" } },
});"#,
        )],
        &["@astrojs/markdoc"],
    );
    let out = output(&results, "astro-markdoc");
    assert_entries(
        &out.production_entries,
        &["src/components/Aside.astro", "src/components/Heading.astro"],
    );
    assert_deps(out, &["@astrojs/markdoc"]);
}

#[test]
fn borp_config_and_command_line() {
    let results = resolve_with_scripts(
        &[(
            ".borp.yaml",
            "files:\n  - 'test/**/*.spec.ts'\nreporters:\n  - spec\n  - '@reporters/github'\n  - ./reporters/custom.js:out.txt\n",
        )],
        &["borp"],
        &[(
            "test",
            "borp --coverage -r gh -r borp-reporter-x --pattern 'it/**/*.test.js' unit/a.test.ts",
        )],
    );
    let out = output(&results, "borp");
    assert_entries(
        &out.entries,
        &[
            "**/*.test.{js,mjs,cjs,ts,mts,cts}",
            "test/**/*.spec.ts",
            "reporters/custom.js",
            "it/**/*.test.js",
            "unit/a.test.ts",
        ],
    );
    assert_deps(out, &["@reporters/github", "borp-reporter-x"]);
    assert_absent(out, &["spec", "gh"]);
}

#[test]
fn ladle_stories_and_vite_config() {
    let results = resolve(
        &[
            (
                ".ladle/config.mjs",
                r#"export default { stories: "components/**/*.stories.tsx", viteConfig: "ladle.vite.config.ts" };"#,
            ),
            (
                "ladle.vite.config.ts",
                "export default { test: { setupFiles: ['./ladle-setup.ts'] } }",
            ),
        ],
        &["@ladle/react"],
    );
    let out = output(&results, "ladle");
    assert_entries(
        &out.entries,
        &["components/**/*.stories.tsx", ".ladle/components.{js,jsx,ts,tsx}"],
    );
    assert!(
        out.config_files
            .iter()
            .any(|path| path.ends_with("ladle.vite.config.ts")),
        "{out:?}"
    );
}

#[test]
fn linthtml_extends_and_plugins() {
    let results = resolve(
        &[(
            ".linthtmlrc.json",
            r#"{"extends":["@linthtml/linthtml-config-recommended","./base.json"],"plugins":["linthtml-plugin-x","./rules/local.js"]}"#,
        )],
        &["@linthtml/linthtml"],
    );
    let out = output(&results, "linthtml");
    assert_deps(out, &["@linthtml/linthtml-config-recommended", "linthtml-plugin-x"]);
    assert_entries(&out.entries, &["rules/local.js"]);
    let manifest = (
        "package.json",
        r#"{"name":"app","linthtmlConfig":{"extends":"linthtml-config-y"}}"#,
    );
    let results = resolve(&[manifest], &["@linthtml/linthtml"]);
    assert_deps(output(&results, "linthtml"), &["linthtml-config-y"]);
}

#[test]
fn lunaria_files_renderer_and_dashboard() {
    let results = resolve(
        &[(
            "docs/lunaria.config.json",
            r#"{"files":[{"location":"src/content/**/*.md","pattern":"x"}],"renderer":"lunaria.renderer.ts","dashboard":{"customCss":["./styles/l.css"],"favicon":{"inline":"<svg/>"}}}"#,
        )],
        &["@lunariajs/core"],
    );
    let out = output(&results, "lunaria");
    assert!(out.config_files.is_empty(), "the config sits at the workspace root");
    let results = resolve(
        &[(
            "lunaria.config.json",
            r#"{"files":[{"location":"src/content/**/*.md","pattern":"x"}],"renderer":"lunaria.renderer.ts","dashboard":{"customCss":["./styles/l.css"],"favicon":{"inline":"<svg/>"}}}"#,
        )],
        &["@lunariajs/core"],
    );
    let out = output(&results, "lunaria");
    assert_entries(
        &out.production_entries,
        &["src/content/**/*.md", "lunaria.renderer.ts", "styles/l.css"],
    );
    assert!(out.production_entries.iter().all(|entry| !entry.contains('<')));
}

#[test]
fn marko_taglibs_and_templates() {
    let results = resolve(
        &[
            ("lib/marko.json", r#"{"taglib-id":"x"}"#),
            (
                "tags/fancy/marko-tag.json",
                r#"{"renderer":"./renderer.js","transform":"marko-transform-x"}"#,
            ),
            ("src/pages/index.marko", "<div/>"),
        ],
        &["marko"],
    );
    let out = output(&results, "marko");
    assert_entries(
        &out.production_entries,
        &["**/{components,tags}/**/*.marko", "lib/**/{components,tags}/**/*.marko"],
    );
    assert_entries(&out.entries, &["tags/fancy/renderer.js"]);
    assert_deps(out, &["marko-transform-x", "marko"]);
    let without_templates = resolve(&[], &["marko"]);
    assert_absent(output(&without_templates, "marko"), &["marko"]);
}

#[test]
fn mcp_server_commands_are_scripts() {
    let results = resolve(
        &[
            (
                ".mcp.json",
                r#"{"mcpServers":{"local":{"command":"node","args":["${CLAUDE_PROJECT_DIR}/tools/mcp.js"]},"remote":{"type":"http","url":"https://x"},"pkg":{"command":"npx","args":["-y","@acme/mcp-server"]}}}"#,
            ),
            (
                ".vscode/mcp.json",
                r#"{"servers":{"svc":{"command":"tsx","args":["server.ts"],"cwd":"${workspaceFolder}/packages/svc"}}}"#,
            ),
            (
                ".codex/config.toml",
                "[mcp_servers.docs]\ncommand = \"node\"\nargs = [\"scripts/docs-mcp.mjs\"]\n",
            ),
        ],
        &[],
    );
    let out = output(&results, "mcp");
    assert_entries(
        &out.entries,
        &["./tools/mcp.js", "packages/svc/server.ts", "scripts/docs-mcp.mjs"],
    );
    assert!(
        out.referenced_dependencies
            .iter()
            .any(|d| d.name == "@acme/mcp-server" && d.optional),
        "{out:?}"
    );
    let nested = resolve_with(&[(".mcp.json", "{}")], &[], false, &IndexMap::new());
    assert!(!nested.iter().any(|(id, _)| *id == "mcp"), "root only");
    let none = resolve(&[], &[]);
    assert!(!none.iter().any(|(id, _)| *id == "mcp"));
}

#[test]
fn mdxlint_plugins_are_remark_packages() {
    let results = resolve(
        &[(
            ".mdxlintrc.json",
            r#"{"plugins":["lint-no-dead-urls",["remark-lint-heading-style","atx"],"mdxlint-plugin-x","@acme/remark-y","./local-plugin.js"]}"#,
        )],
        &["mdxlint"],
    );
    let out = output(&results, "mdxlint");
    assert_deps(
        out,
        &[
            "remark-lint-no-dead-urls",
            "remark-lint-heading-style",
            "mdxlint-plugin-x",
            "@acme/remark-y",
        ],
    );
    assert_entries(&out.entries, &["local-plugin.js"]);
}

#[test]
fn npm_package_json_lint_extends() {
    let results = resolve(
        &[(
            ".npmpackagejsonlintrc.json",
            r#"{"extends":"npm-package-json-lint-config-default","rules":{}}"#,
        )],
        &["npm-package-json-lint"],
    );
    let out = output(&results, "npm-package-json-lint");
    assert_deps(out, &["npm-package-json-lint-config-default"]);
    assert!(out.provided_binaries.contains(&"npmPkgJsonLint".to_owned()));
    let manifest = (
        "package.json",
        r#"{"name":"app","npmpackagejsonlint":{"extends":["@acme/npmpkgjsonlint-config"]}}"#,
    );
    let results = resolve(&[manifest], &["npm-package-json-lint"]);
    assert_deps(
        output(&results, "npm-package-json-lint"),
        &["@acme/npmpkgjsonlint-config"],
    );
}

#[test]
fn osls_reads_serverless_config() {
    let results = resolve(
        &[(
            "serverless.yml",
            "service: x\nplugins:\n  - serverless-offline\nfunctions:\n  api:\n    handler: src/api.handler\n",
        )],
        &["osls"],
    );
    let out = output(&results, "osls");
    assert_deps(out, &["serverless-offline"]);
    assert!(out.entries.iter().any(|entry| entry.starts_with("src/api")), "{out:?}");
}

#[test]
fn playwright_test_first_positional_is_an_entry() {
    let results = resolve_with_scripts(
        &[],
        &["playwright-test"],
        &[
            ("test", "playwright-test test/browser.test.js --runner mocha"),
            ("glob", "pw-test 'test/**/*.spec.js' --mode worker"),
            ("install", "playwright-test install"),
        ],
    );
    let out = output(&results, "playwright-test");
    assert_entries(&out.entries, &["test/browser.test.js", "test/**/*.spec.js"]);
    assert!(!out.entries.iter().any(|entry| entry == "install"), "{:?}", out.entries);
}

#[test]
fn raycast_commands_and_tools() {
    let manifest = (
        "package.json",
        r#"{"name":"ext","commands":[{"name":"search","mode":"view"},{"name":"../x"}],"tools":[{"name":"lookup"}],"dependencies":{"@raycast/api":"1"}}"#,
    );
    let results = resolve(&[manifest], &["@raycast/api"]);
    let out = output(&results, "raycast");
    assert_entries(
        &out.production_entries,
        &["src/search.{js,jsx,ts,tsx}", "src/tools/lookup.{js,jsx,ts,tsx}"],
    );
    assert_eq!(out.production_entries.len(), 2, "{:?}", out.production_entries);
}

#[test]
fn react_cosmos_fixtures_dir_and_suffix() {
    let results = resolve(
        &[(
            "cosmos.config.json",
            r#"{"fixturesDir":"cosmos","fixtureFileSuffix":"story","plugins":["react-cosmos-plugin-vite"]}"#,
        )],
        &["react-cosmos"],
    );
    let out = output(&results, "react-cosmos");
    assert_entries(
        &out.entries,
        &[
            "cosmos/**/*.{js,jsx,ts,tsx,md,mdx}",
            "cosmos/**/*.story.{js,jsx,ts,tsx,md,mdx}",
            "**/*.fixture.{js,jsx,ts,tsx,md,mdx}",
        ],
    );
    assert_deps(out, &["react-cosmos-plugin-vite"]);
}

#[test]
fn tsrs_reads_tsconfig() {
    let results = resolve(
        &[(
            "tsconfig.json",
            r#"{"extends":"@tsconfig/node20/tsconfig.json","compilerOptions":{"types":["node"]}}"#,
        )],
        &["tsrs"],
    );
    let out = output(&results, "tsrs");
    assert_deps(out, &["@tsconfig/node20"]);
    assert!(out.provided_binaries.contains(&"tsrs".to_owned()));
}

#[test]
fn varlock_decorators_of_env_files() {
    let manifest = ("package.json", r#"{"name":"app","varlock":{"loadPath":"./env/"}}"#);
    let results = resolve_with_scripts(
        &[
            manifest,
            (
                "env/.env.schema",
                "# @plugin(@varlock/1password-plugin)\n# @plugin(./plugins/custom.ts)\n# @import(../shared/.env.shared)\n# ---\n\n# @sensitive\nAPI_KEY=\n",
            ),
            ("shared/.env.shared", "# @plugin(varlock-plugin-shared)\n\nX=1\n"),
            ("config/.env.ci", "# @plugin(varlock-plugin-ci, enabled=false)\n\nY=1\n"),
            (
                "config/.env.extra",
                "# @plugin(varlock-plugin-extra)\n# ---\n# @plugin(not-root)\nZ=1\n",
            ),
            (".env", "# @plugin(varlock-plugin-default)\n"),
        ],
        &["varlock"],
        &[
            ("start", "varlock run -- node server.js"),
            ("ci", "varlock load --path config/"),
        ],
    );
    let out = output(&results, "varlock");
    assert_deps(
        out,
        &[
            "@varlock/1password-plugin",
            "varlock-plugin-shared",
            "varlock-plugin-extra",
        ],
    );
    assert_absent(out, &["varlock-plugin-ci", "varlock-plugin-default", "not-root"]);
    assert_entries(&out.production_entries, &["env/plugins/custom.ts"]);
    assert_entries(&out.entries, &["server.js"]);
    let defaults = resolve(&[(".env", "# @plugin(varlock-plugin-default)\n")], &["varlock"]);
    assert_deps(output(&defaults, "varlock"), &["varlock-plugin-default"]);
}

#[test]
fn yorkie_git_hooks_are_scripts() {
    let manifest = (
        "package.json",
        r#"{"name":"app","gitHooks":{"pre-commit":"lint-staged","commit-msg":"node scripts/verify-commit.js"}}"#,
    );
    let results = resolve(&[manifest], &["yorkie"]);
    let out = output(&results, "yorkie");
    assert!(out.referenced_binaries.contains(&"lint-staged".to_owned()));
    assert_entries(&out.entries, &["scripts/verify-commit.js"]);
    assert_deps(out, &["yorkie"]);
}

#[test]
fn filesystem_routing_dir_and_extensions() {
    let results = resolve(
        &[(
            "vite.config.ts",
            r#"import routes from "filesystem-routing/vite";
export default { root: "app", plugins: [routes({ dir: "pages", extensions: ["tsx"] })] };"#,
        )],
        &["filesystem-routing"],
    );
    assert_entries(
        &output(&results, "filesystem-routing").production_entries,
        &["app/pages/**/*.tsx"],
    );
    let results = resolve(
        &[("vite.config.ts", "export default { root: 'web' };")],
        &["filesystem-routing"],
    );
    assert_entries(
        &output(&results, "filesystem-routing").production_entries,
        &["web/src/routes/**/*.{js,jsx,ts,tsx,tsrx}"],
    );
}

#[test]
fn flue_modules_and_agents() {
    let results = resolve(
        &[
            (
                "flue.config.ts",
                r#"export default { app: "./server/app.ts", agents: "bots/*.ts" };"#,
            ),
            ("src/db.ts", ""),
        ],
        &["@flue/runtime"],
    );
    let out = output(&results, "flue");
    assert_entries(
        &out.production_entries,
        &["server/app.ts", "src/db.{ts,mts,js,mjs}", "src/bots/*.ts"],
    );
}

#[test]
fn laravel_vite_plugin_inputs() {
    let results = resolve(
        &[(
            "vite.config.js",
            r#"import { defineConfig } from "vite";
import laravel from "laravel-vite-plugin";
export default defineConfig({ plugins: [laravel({ input: ["resources/css/app.css", "resources/js/app.js"], ssr: "resources/js/ssr.js" })] });"#,
        )],
        &["laravel-vite-plugin"],
    );
    assert_entries(
        &output(&results, "laravel-vite-plugin").production_entries,
        &["resources/css/app.css", "resources/js/app.js", "resources/js/ssr.js"],
    );
}

#[test]
fn railway_service_commands() {
    let manifest = ("package.json", r#"{"name":"app","repository":"github:acme/app"}"#);
    let results = resolve(
        &[
            manifest,
            (
                ".railway/railway.ts",
                r#"import { service } from "railway/iac";
service("api", { root: "services/api", build: "tsc -b", start: "node dist/server.js", deploy: { preDeployCommand: ["node migrate.js"] } });
service("web", { source: { repo: "acme/app" }, start: "node web.js" });
service("db", { source: { image: "postgres:16" }, start: "node never.js" });
service("other", { source: { repo: "https://github.com/else/where" }, start: "node elsewhere.js" });"#,
            ),
        ],
        &["railway"],
    );
    let out = output(&results, "railway");
    assert_entries(
        &out.entries,
        &["services/api/dist/server.js", "services/api/migrate.js", "web.js"],
    );
    for absent in ["never.js", "elsewhere.js"] {
        assert!(
            !out.entries.iter().any(|entry| entry == absent),
            "{absent}: {:?}",
            out.entries
        );
    }
}

#[test]
fn solidjs_vite_plugin_start_entries() {
    let results = resolve(
        &[
            (
                "vite.config.ts",
                r#"import { solid } from "@solidjs/vite-plugin";
export default { plugins: [solid({ ssr: true, start: { app: "./src/root.tsx", middleware: "./src/middleware.ts" }, serverFunctions: { configure: "./src/fns.ts" } })] };"#,
            ),
            ("src/entry-server.tsx", ""),
        ],
        &["@solidjs/vite-plugin"],
    );
    let out = output(&results, "solidjs-vite-plugin");
    assert_entries(
        &out.production_entries,
        &[
            "src/root.tsx",
            "src/middleware.ts",
            "src/fns.ts",
            "src/Document.{tsx,jsx,tsrx}",
            "src/entry-server.{tsx,jsx,ts,js,mjs,tsrx}",
        ],
    );
}

#[test]
fn vite_plugin_pages_and_layouts_dirs() {
    let config = (
        "vite.config.ts",
        r#"import Pages from "vite-plugin-pages";
import Layouts from "vite-plugin-vue-layouts-next";
export default { plugins: [Pages({ dirs: ["src/views", { dir: "src/admin", baseRoute: "admin" }] }), Layouts({ layoutsDirs: "src/shells" })] };"#,
    );
    let results = resolve(&[config], &["vite-plugin-pages", "vite-plugin-vue-layouts-next"]);
    let pages = output(&results, "vite-plugin-pages");
    assert_entries(
        &pages.production_entries,
        &[
            "src/views/**/*.{vue,jsx,tsx,md,mdx}",
            "src/admin/**/*.{vue,jsx,tsx,md,mdx}",
        ],
    );
    assert!(pages.virtual_modules.contains(&"~pages".to_owned()));
    assert_entries(
        &output(&results, "vite-plugin-vue-layouts-next").production_entries,
        &["src/shells/**/*.vue"],
    );
}

#[test]
fn vite_plus_tasks_and_staged_commands() {
    let results = resolve(
        &[(
            "vite.config.ts",
            r#"import { defineConfig } from "vite-plus";
export default defineConfig({
  run: { tasks: { gen: { command: "node scripts/gen.js" }, check: { command: ["tsc --noEmit"] } } },
  staged: { "*.ts": "eslint --fix" },
});"#,
        )],
        &["vite-plus"],
    );
    let out = output(&results, "vite-plus");
    assert_entries(&out.entries, &["scripts/gen.js"]);
    for binary in ["tsc", "eslint"] {
        assert!(out.referenced_binaries.contains(&binary.to_owned()), "{binary}");
    }
}

#[test]
fn openclaw_is_enabled_by_its_manifest_key() {
    let manifest = (
        "package.json",
        r#"{"name":"plugin","openclaw":{"extensions":["./src/index.ts"],"setupEntry":"./src/setup.ts","channel":{"configuredState":{"specifier":"./src/state.ts"}},"hooks":["./hooks/on-message/"],"build":{"staticAssets":[{"source":"./assets/logo.svg"}]},"assetScripts":{"build":"node scripts/assets.js"}}}"#,
    );
    let results = resolve(
        &[
            manifest,
            (
                "openclaw.plugin.json",
                r#"{"id":"x","providers":[{"providerCatalogEntry":"./src/catalog.ts"}]}"#,
            ),
        ],
        &[],
    );
    let out = output(&results, "openclaw");
    assert_entries(
        &out.production_entries,
        &[
            "src/index.ts",
            "src/setup.ts",
            "src/state.ts",
            "hooks/on-message/{handler,index}.{ts,js}",
            "assets/logo.svg",
            "src/catalog.ts",
        ],
    );
    assert_entries(&out.entries, &["scripts/assets.js"]);
    let none = resolve(&[], &[]);
    assert!(!none.iter().any(|(id, _)| *id == "openclaw"));
}

#[test]
fn typescript_content_mapper_exec_is_a_script() {
    let manifest = (
        "package.json",
        r#"{"name":"app","typescript":{"contentMapper":{"exec":["node","scripts/map content.js"]}}}"#,
    );
    let results = resolve(&[manifest], &[]);
    let out = output(&results, "typescript-content-mapper");
    assert_entries(&out.entries, &["scripts/map content.js"]);
    let plain = ("package.json", r#"{"name":"app","typescript":{"other":true}}"#);
    let results = resolve(&[plain], &[]);
    assert!(!results.iter().any(|(id, _)| *id == "typescript-content-mapper"));
}
