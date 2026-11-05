//! False positives caused by configs the plugins did not read: computed configs, configs outside the package root,
//! entries computed with glob calls, and scripts that point tools at their config.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::Path;

use indexmap::IndexMap;
use skif_core::MemoryFs;
use skif_js_plugins::{PluginContext, PluginId, PluginOutput, resolve_plugins};

const DIR: &str = "/ws";

fn run(files: &[(&str, &str)], dependencies: &[&str], scripts: &[(&str, &str)]) -> Vec<(PluginId, PluginOutput)> {
    let mut fs = MemoryFs::new();
    fs.insert(format!("{DIR}/package.json"), r#"{"name":"app"}"#);
    for (path, content) in files {
        fs.insert(format!("{DIR}/{path}"), content);
    }
    let dependencies: Vec<String> = dependencies.iter().map(|d| (*d).to_string()).collect();
    let scripts: IndexMap<String, String> = scripts
        .iter()
        .map(|(name, body)| ((*name).to_string(), (*body).to_string()))
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

fn output<'a>(results: &'a [(PluginId, PluginOutput)], id: &str) -> &'a PluginOutput {
    results
        .iter()
        .find(|(plugin, _)| *plugin == id)
        .map_or_else(|| panic!("plugin {id} is not enabled"), |(_, output)| output)
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

#[test]
fn webpack_config_function_with_local_bindings_and_template_loader() {
    let config = r"
module.exports = async (env) => {
  const loaderPath = require.resolve('babel-loader');
  const rule = (test, use) => ({ test, use });
  const config = {
    entry: './src/index.js',
    module: { rules: [rule(/\.js$/, `${loaderPath}?cacheDirectory`), { test: /\.css$/, use: ['style-loader', 'css-loader'] }] },
  };
  return config;
};";
    let results = run(&[("webpack.config.js", config)], &["webpack"], &[]);
    let out = output(&results, "webpack");
    assert_deps(out, &["babel-loader", "style-loader", "css-loader"]);
    assert_entries(&out.production_entries, &["src/index.js"]);
    assert!(!out.entries_incomplete);
}

#[test]
fn glob_pattern_literals_become_bundler_entries() {
    let cases = [
        ("import { glob } from 'glob';", "await glob('src/*/index.{ts,tsx}')"),
        ("import fg from 'fast-glob';", "fg.sync('src/*/index.{ts,tsx}')"),
        (
            "import { globSync } from 'tinyglobby';",
            "globSync(['src/*/index.{ts,tsx}'])",
        ),
    ];
    for (import, call) in cases {
        let config = format!(
            "{import}\nexport default async () => {{\n  const files = {call};\n  return {{ entry: files }};\n}};"
        );
        let results = run(&[("webpack.config.mjs", &config)], &["webpack"], &[]);
        let out = output(&results, "webpack");
        assert_entries(&out.production_entries, &["src/*/index.{ts,tsx}"]);
        assert!(!out.entries_incomplete, "{call}");
    }
}

#[test]
fn computed_entries_are_flagged_as_incomplete() {
    let cases = [
        (
            "webpack",
            "webpack.config.js",
            "module.exports = { entry: readEntries() };",
        ),
        (
            "webpack",
            "webpack.config.js",
            "module.exports = { ...shared, mode: 'x' };",
        ),
        ("webpack", "webpack.config.js", "module.exports = makeConfig();"),
        ("rollup", "rollup.config.js", "export default { input: pick() };"),
        ("tsup", "tsup.config.ts", "export default { entry: pick() };"),
        ("tsdown", "tsdown.config.ts", "export default { entry: pick() };"),
        (
            "esbuild",
            "esbuild.config.mjs",
            "export default { entryPoints: pick() };",
        ),
        (
            "vite",
            "vite.config.ts",
            "export default { build: { rollupOptions: { input: pick() } } };",
        ),
    ];
    for (plugin, file, source) in cases {
        let results = run(&[(file, source)], &[plugin], &[]);
        assert!(output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

#[test]
fn static_entries_are_complete() {
    let cases = [
        (
            "webpack",
            "webpack.config.js",
            "module.exports = { entry: { main: './src/a.js' } };",
        ),
        ("rollup", "rollup.config.js", "export default { input: 'src/a.js' };"),
        ("tsup", "tsup.config.ts", "export default { entry: ['src/a.ts'] };"),
        ("tsdown", "tsdown.config.ts", "export default { entry: ['src/a.ts'] };"),
        (
            "esbuild",
            "esbuild.config.mjs",
            "export default { entryPoints: ['src/a.ts'] };",
        ),
        (
            "vite",
            "vite.config.ts",
            "export default { build: { lib: { entry: 'src/a.ts' } } };",
        ),
    ];
    for (plugin, file, source) in cases {
        let results = run(&[(file, source)], &[plugin], &[]);
        assert!(!output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

#[test]
fn storybook_get_absolute_path_idiom_names_the_packages() {
    let main = r"
const { dirname, join } = require('node:path');
function getAbsolutePath(value) { return dirname(require.resolve(join(value, 'package.json'))); }
module.exports = {
  stories: ['../src/**/*.stories.tsx'],
  addons: [getAbsolutePath('@storybook/addon-links'), getAbsolutePath('@storybook/addon-essentials')],
  framework: { name: getAbsolutePath('@storybook/react-vite'), options: {} },
};";
    let results = run(&[(".storybook/main.cjs", main)], &["@storybook/react"], &[]);
    assert_deps(
        output(&results, "storybook"),
        &[
            "@storybook/addon-links",
            "@storybook/addon-essentials",
            "@storybook/react-vite",
        ],
    );
}

#[test]
fn vite_css_postcss_file_is_read_as_a_postcss_config() {
    let results = run(
        &[
            (
                "vite.config.ts",
                "export default { css: { postcss: './config/postcss.config.js' } };",
            ),
            (
                "config/postcss.config.js",
                "export default { plugins: { '@tailwindcss/postcss': {}, autoprefixer: {} } };",
            ),
            (
                "config/tailwind.config.ts",
                "export default { plugins: ['tailwindcss-animate'] };",
            ),
        ],
        &["vite", "tailwindcss"],
        &[],
    );
    let vite = output(&results, "vite");
    assert_deps(vite, &["@tailwindcss/postcss", "autoprefixer"]);
    assert_entries(&vite.entries, &["config/postcss.config.js"]);
    let tailwind = output(&results, "tailwind");
    assert_entries(
        &tailwind.entries,
        &["{,config/}tailwind.config.{js,cjs,mjs,ts,cts,mts}"],
    );
    assert_deps(tailwind, &["tailwindcss-animate"]);
}

#[test]
fn vite_css_postcss_directory_and_inline_object() {
    let results = run(
        &[
            ("vite.config.ts", "export default { css: { postcss: './build' } };"),
            (
                "build/postcss.config.cjs",
                "module.exports = { plugins: [['postcss-nested', {}]] };",
            ),
        ],
        &["vite"],
        &[],
    );
    assert_deps(output(&results, "vite"), &["postcss-nested"]);

    let results = run(
        &[(
            "vite.config.ts",
            "export default { css: { postcss: { plugins: { 'postcss-preset-env': {} } } } };",
        )],
        &["vite"],
        &[],
    );
    assert_deps(output(&results, "vite"), &["postcss-preset-env"]);
}

#[test]
fn postcss_and_tailwind_configs_in_a_config_directory_are_found() {
    let results = run(
        &[
            (
                "config/postcss.config.js",
                "export default { plugins: { autoprefixer: {} } };",
            ),
            (
                "config/tailwind.config.js",
                "export default { presets: ['tw-preset'] };",
            ),
        ],
        &["postcss", "tailwindcss"],
        &[],
    );
    assert_deps(output(&results, "postcss"), &["autoprefixer"]);
    assert_deps(output(&results, "tailwind"), &["tw-preset"]);
}

#[test]
fn storybook_config_directory_from_a_script_flag() {
    let results = run(
        &[
            (
                "ui/.storybook/main.ts",
                "export default { addons: ['@storybook/addon-a11y'], framework: '@storybook/react-vite' };",
            ),
            ("ui/.storybook/preview.ts", "export default {};"),
        ],
        &["@storybook/react"],
        &[("storybook", "storybook dev -p 6006 -c ui/.storybook")],
    );
    let out = output(&results, "storybook");
    assert_deps(out, &["@storybook/addon-a11y", "@storybook/react-vite"]);
    assert_entries(
        &out.entries,
        &["ui/.storybook/{manager,preview,index,vitest.setup}.{js,jsx,ts,tsx}"],
    );
    assert!(out.config_files.iter().any(|f| f.ends_with("ui/.storybook/main.ts")));
}

#[test]
fn config_files_from_script_flags() {
    let results = run(
        &[(
            "ui/vitest.config.ts",
            "export default { test: { environment: 'jsdom', coverage: { provider: 'v8' } } };",
        )],
        &["vitest"],
        &[
            ("test", "vitest run --config ui/vitest.config.ts"),
            ("other", "vitest --config=../elsewhere.ts"),
        ],
    );
    let out = output(&results, "vitest");
    assert_deps(out, &["jsdom", "@vitest/coverage-v8"]);
    assert!(out.config_files.iter().any(|f| f.ends_with("ui/vitest.config.ts")));
}

#[test]
fn playwright_test_match_regular_expression() {
    let results = run(
        &[(
            "playwright.config.js",
            r"module.exports = { testMatch: /.*\.setup\.js/ };",
        )],
        &["@playwright/test"],
        &[],
    );
    assert_entries(&output(&results, "playwright").entries, &["**/*.setup.js*"]);

    let results = run(
        &[(
            "playwright.config.js",
            "module.exports = { testDir: 'e2e', testMatch: [/a+b/, 'x.spec.js'] };",
        )],
        &["@playwright/test"],
        &[],
    );
    assert_entries(
        &output(&results, "playwright").entries,
        &["e2e/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}", "e2e/x.spec.js"],
    );
}

#[test]
fn aws_cdk_lambda_entries_and_app_commands() {
    let stack = r"
import * as path from 'path';
import { NodejsFunction } from 'aws-cdk-lib/aws-lambda-nodejs';
export class Stack {
  constructor(scope) {
    new NodejsFunction(this, 'x', { entry: path.join(__dirname, 'lambda/x.ts'), handler: 'main' });
    new lambda_nodejs.NodejsFunction(this, 'y', { entry: path.join(__dirname, '..', 'functions', 'y.ts') });
  }
}";
    let results = run(
        &[
            ("lib/stack.ts", stack),
            (
                "cdk.out/asset/ignored.ts",
                "new NodejsFunction(this, 'z', { entry: 'z.ts' });",
            ),
        ],
        &["aws-cdk-lib"],
        &[("deploy", "cdk deploy --app \"npx ts-node bin/app.ts\" --all")],
    );
    let out = output(&results, "aws-cdk");
    assert_entries(&out.entries, &["lib/lambda/x.ts", "functions/y.ts", "bin/app.ts"]);
    assert!(!out.entries.iter().any(|e| e.contains("cdk.out")), "{:?}", out.entries);
    assert_deps(out, &["esbuild"]);
    assert!(!out.entries_incomplete);

    let results = run(
        &[("lib/stack.ts", "new NodejsFunction(this, 'x', { entry: pick() });")],
        &["aws-cdk-lib"],
        &[],
    );
    assert!(output(&results, "aws-cdk").entries_incomplete);
}

#[test]
fn github_actions_steps_in_other_directories_are_scripts_of_that_directory() {
    let workflow = r#"on: [push]
defaults:
  run:
    working-directory: infra
jobs:
  a:
    steps:
      - run: node ./post-release.js
        working-directory: tools
      - run: node ./deploy.js
      - run: npx cdk deploy -a "npx ts-node extra/main.ts"
        working-directory: ./backend/
      - run: node ./skipped.js
        working-directory: ../outside
      - run: node ./computed.js
        working-directory: ${{ matrix.dir }}
      - run: node ./root.js
        working-directory: .
"#;
    let results = run(&[(".github/workflows/ci.yml", workflow)], &[], &[]);
    let out = output(&results, "github-actions");
    assert_entries(
        &out.entries,
        &[
            "tools/post-release.js",
            "infra/deploy.js",
            "backend/extra/main.ts",
            "./root.js",
        ],
    );
    for skipped in ["skipped.js", "computed.js"] {
        assert!(
            !out.entries.iter().any(|e| e.ends_with(skipped)),
            "{skipped}: {:?}",
            out.entries
        );
    }
}

#[test]
fn browser_extension_manifest_below_the_root_and_index_sources() {
    let manifest = r#"{
      "manifest_version": 3,
      "background": {"service_worker": "dist/background.js"},
      "options_page": "settings.html",
      "content_scripts": [{"js": ["dist/content.cjs"]}]
    }"#;
    let results = run(
        &[
            ("public/manifest.json", manifest),
            ("src/background/index.ts", ""),
            ("public/settings.html", r#"<script src="dist/settings.js"></script>"#),
        ],
        &[],
        &[],
    );
    let out = output(&results, "browser-extension");
    assert_entries(
        &out.entries,
        &[
            "src/background/index.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "src/background.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "src/content.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
            "src/settings/index.{js,jsx,ts,tsx,mjs,mts,cjs,cts}",
        ],
    );
}

#[test]
fn browser_extension_manifest_in_a_first_level_directory() {
    let manifest = r#"{"manifest_version": 3, "background": {"service_worker": "worker.js"}}"#;
    let results = run(&[("chrome-extension/manifest.json", manifest)], &[], &[]);
    assert_entries(
        &output(&results, "browser-extension").entries,
        &["chrome-extension/worker.{js,jsx,ts,tsx,mjs,mts,cjs,cts}"],
    );
}

#[test]
fn jsx_react_jsx_in_a_base_tsconfig_is_not_a_react_dependency() {
    let results = run(
        &[
            (
                "tsconfig.json",
                r#"{"extends":"./tsconfig.base.json","compilerOptions":{"outDir":"dist"}}"#,
            ),
            (
                "tsconfig.base.json",
                r#"{"compilerOptions":{"jsx":"react-jsx","strict":true}}"#,
            ),
        ],
        &["typescript"],
        &[],
    );
    let out = output(&results, "typescript");
    assert!(!deps(out).contains("react"), "{:?}", deps(out));
    assert!(out.config_files.iter().any(|f| f.ends_with("tsconfig.base.json")));
}

#[test]
fn an_unreadable_config_marks_the_entries_of_any_plugin_incomplete() {
    let cases = [
        ("vitest", "vitest.config.ts", "export default makeConfig();"),
        ("vitest", "vitest.config.ts", "export default { ...shared, test: { include: ['a'] } };"),
        ("jest", "jest.config.js", "module.exports = loadConfig();"),
        ("playwright", "playwright.config.ts", "export default { ...base, testDir: './e2e' };"),
        ("eslint", "eslint.config.js", "export default makeConfig();"),
        ("prettier", ".prettierrc.json", "{ not json at all"),
    ];
    for (plugin, file, source) in cases {
        let results = run(&[(file, source)], &[plugin], &[]);
        assert!(output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

#[test]
fn a_readable_config_leaves_the_entries_complete() {
    let cases = [
        ("vitest", "vitest.config.ts", "export default { test: { include: ['a'] } };"),
        ("jest", "jest.config.js", "module.exports = { testMatch: ['**/*.t.js'] };"),
        ("playwright", "playwright.config.ts", "export default { testDir: './e2e' };"),
    ];
    for (plugin, file, source) in cases {
        let results = run(&[(file, source)], &[plugin], &[]);
        assert!(!output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

#[test]
fn merge_config_keeps_the_known_part_and_is_incomplete_for_the_imported_one() {
    let source = "import viteConfig from './vite.config';\n\
                  export default mergeConfig(viteConfig, defineConfig({ test: { setupFiles: ['./config/setup.ts'] } }));";
    let results = run(&[("vitest.config.ts", source)], &["vitest"], &[]);
    let out = output(&results, "vitest");
    assert_entries(&out.entries, &["config/setup.ts"]);
    assert!(out.entries_incomplete);
}
