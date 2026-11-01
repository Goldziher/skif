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

const ADR_IDS: [&str; 21] = [
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
    "rollup",
    "tsup",
    "turbo",
    "husky",
    "lint-staged",
    "commitlint",
    "changesets",
    "github-actions",
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
    let fs = fs_with(files);
    let dependencies: Vec<String> = dependencies.iter().map(|d| (*d).to_string()).collect();
    let scripts = IndexMap::new();
    let ctx = PluginContext {
        dir: Path::new(DIR),
        is_root,
        package_name: Some("app"),
        dependencies: &dependencies,
        scripts: &scripts,
        fs: &fs,
        plugins,
    };
    resolve_plugins(&ctx)
}

fn resolve(files: &[(&str, &str)], dependencies: &[&str]) -> Vec<(PluginId, PluginOutput)> {
    resolve_with(files, dependencies, true, &IndexMap::new())
}

fn output<'a>(results: &'a [(PluginId, PluginOutput)], id: &str) -> &'a PluginOutput {
    results
        .iter()
        .find(|(plugin, _)| *plugin == id)
        .map(|(_, output)| output)
        .unwrap_or_else(|| panic!("plugin {id} is not enabled; enabled: {:?}", results.iter().map(|(i, _)| i).collect::<Vec<_>>()))
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
        assert!(entries.iter().any(|e| e == pattern), "missing entry {pattern}; got {entries:?}");
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
    assert!(resolve(&[], &[]).is_empty());
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
            ("packages/a/tsconfig.json", r#"{"compilerOptions":{"importHelpers":true}}"#),
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
        &[("package.json", r#"{"name":"app","eslintConfig":{"extends":["next/core-web-vitals"]}}"#)],
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
        &[("package.json", r#"{"name":"app","prettier":"@company/prettier-config"}"#)],
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
            ("index.html", r#"<html><script type="module" src="/src/index.tsx"></script></html>"#),
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
        &["jsdom", "vitest-sonar-reporter", "@vitest/coverage-istanbul", "vitest-canvas-mock"],
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
        &[("next.config.mjs", r#"export default { pageExtensions: ["tsx", "ts", "mdx"] };"#)],
        &["next"],
    );
    let out = output(&results, "next");
    assert_entries(&out.entries, &["next-env.d.ts", "{,src/}pages/**/*.{tsx,ts,mdx}"]);
    assert_entries(&out.production_entries, &["{,src/}pages/**/*.{tsx,ts,mdx}"]);
    assert!(out.entries.iter().any(|e| e.contains("app/**/{layout,page,route,template}")));
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
    assert!(out.referenced_dependencies.iter().any(|d| d.name == "@vitest/coverage-v8" && d.optional));
}

#[test]
fn storybook_defaults_to_conventional_stories() {
    let results = resolve(&[(".storybook/main.ts", "export default {};")], &["@storybook/react"]);
    assert!(output(&results, "storybook").entries.iter().any(|e| e.contains("stories")));
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
        &["e2e/**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}", "e2e/smoke/*.spec.ts", "e2e/global-setup.ts"],
    );
    assert_deps(out, &["playwright-allure"]);
    assert!(out.referenced_binaries.contains(&"vite".to_string()));
}

#[test]
fn tailwind() {
    let results = resolve(
        &[("tailwind.config.js", r#"module.exports = { content: [], plugins: ["@tailwindcss/typography"] };"#)],
        &["tailwindcss"],
    );
    let out = output(&results, "tailwind");
    assert_deps(out, &["@tailwindcss/typography"]);
    assert_entries(&out.entries, &["tailwind.config.{js,cjs,mjs,ts,cts,mts}"]);
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
    assert_deps(out, &["ts-loader", "style-loader", "css-loader", "babel-loader", "@babel/preset-env"]);
}

#[test]
fn rollup() {
    let results = resolve(
        &[("rollup.config.mjs", r#"export default [{ input: "src/a.js" }, { input: { b: "src/b.js" } }];"#)],
        &["rollup"],
    );
    assert_entries(&output(&results, "rollup").production_entries, &["src/a.js", "src/b.js"]);
}

#[test]
fn tsup() {
    let results = resolve(
        &[("tsup.config.ts", r#"export default defineConfig({ entry: ["src/index.ts", "!src/skip.ts"] });"#)],
        &["tsup"],
    );
    let out = output(&results, "tsup");
    assert_entries(&out.production_entries, &["src/index.ts"]);
    assert!(!out.entries.iter().any(|e| e.contains("skip")));
}

#[test]
fn tsup_config_in_package_json() {
    let results = resolve(
        &[("package.json", r#"{"name":"app","tsup":{"entry":{"cli":"src/cli.ts"}}}"#)],
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
    assert!(out.referenced_binaries.contains(&"lint-staged".to_string()), "{:?}", out.referenced_binaries);
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
        assert!(out.referenced_binaries.contains(&binary.to_string()), "{binary}: {:?}", out.referenced_binaries);
    }
}

#[test]
fn lint_staged_rc_file() {
    let results = resolve(&[(".lintstagedrc.json", r#"{"*.js":"eslint --fix"}"#)], &["lint-staged"]);
    assert!(output(&results, "lint-staged").referenced_binaries.contains(&"eslint".to_string()));
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
    assert_deps(output(&results, "changesets"), &["@changesets/changelog-github", "@changesets/config"]);
}

#[test]
fn github_actions() {
    let workflow = "name: ci\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: pnpm exec vitest run\n      - run: |\n          npx playwright test\n      - run: npx other-tool\n        working-directory: packages/x\n";
    let action = "name: local\nruns:\n  using: node20\n  main: dist/index.js\n";
    let results = resolve(
        &[(".github/workflows/ci.yml", workflow), (".github/actions/local/action.yml", action)],
        &[],
    );
    let out = output(&results, "github-actions");
    assert!(out.referenced_binaries.contains(&"vitest".to_string()), "{:?}", out.referenced_binaries);
    assert!(out.referenced_binaries.contains(&"playwright".to_string()));
    assert!(!out.referenced_binaries.contains(&"other-tool".to_string()), "steps in another directory are skipped");
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
            ("playwright.config.ts", r#"export default { globalSetup: "./other.ts" };"#),
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
    assert!(results.is_empty());
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
    let broken_manifest = [("package.json", r#"{"name":1,"eslintConfig":5,"jest":[],"lint-staged":"x","husky":null}"#)];
    let results = resolve_with(&broken_manifest, &[], true, &plugins);
    assert_eq!(results.len(), ADR_IDS.len());
}
