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

const ADR_IDS: [&str; 63] = [
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
        PluginConfig::Options(PluginOptions {
            config: vec!["e2e.config.ts".to_string()],
            entry: vec!["e2e/**/*.ts".to_string()],
            project: vec!["e2e/**".to_string()],
            ..PluginOptions::default()
        }),
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
        PluginConfig::Options(PluginOptions {
            enabled: Some(false),
            ..PluginOptions::default()
        }),
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
            "src/routes/**/*.{js,jsx,ts,tsx}",
        ],
    );
    let config = r#"{ "generatedRouteTree": "./app/tree.gen.ts", "routesDirectory": "./app/pages" }"#;
    let results = resolve(&[("tsr.config.json", config)], &["@tanstack/react-router"]);
    assert_entries(
        &output(&results, "tanstack-router").production_entries,
        &["app/tree.gen.ts", "app/pages/**/*.{js,jsx,ts,tsx}"],
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
