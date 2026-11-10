//! False positives caused by configs the plugins did not read: computed configs, configs outside the package root,
//! entries computed with glob calls, and scripts that point tools at their config.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::Path;

use indexmap::IndexMap;
use skif_core::MemoryFs;
use skif_js_plugins::{IncompleteCause, IncompleteScope, PluginContext, PluginId, PluginOutput, resolve_plugins};

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
        modules: None,
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
        let dependency = if plugin == "playwright" {
            "@playwright/test"
        } else {
            plugin
        };
        let results = run(&[(file, source)], &[dependency], &[]);
        assert!(output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

/// A library build whose entries a helper computes from a directory (mastra's `playground-ui` builds
/// `./components/*` from `src/ds/components` with `...componentEntries()`): the static entries are kept and the rest
/// are unknown.
#[test]
fn a_spread_in_lib_entries_is_incomplete() {
    let helper = "import { readdirSync } from 'node:fs';\nconst componentEntries = (directory: string, prefix: string) => {\n  const entries: Array<[string, string]> = [];\n  readdirSync(directory).forEach(name => entries.push([`${prefix}/${name}`, `${directory}/${name}/index.ts`]));\n  return Object.fromEntries(entries);\n};\n";
    let spread = format!(
        "{helper}export default {{ build: {{ lib: {{ entry: {{ tokens: 'src/ds/tokens/index.ts', ...componentEntries('src/ds/components', 'components') }} }} }} }};"
    );
    let results = run(&[("vite.config.ts", &spread)], &["vite"], &[]);
    let out = output(&results, "vite");
    assert!(out.entries_incomplete);
    assert_entries(&out.production_entries, &["src/ds/tokens/index.ts"]);
    // The spread is inside the entry key, so the config computes the key itself: any file may be an entry.
    assert!(package_wide(&results, "vite"), "{:?}", out.incomplete_reasons);

    let conditional = format!(
        "{helper}import {{ defineConfig }} from 'vite';\nconst lib = (production: boolean) => ({{ build: {{ emptyOutDir: production, lib: {{ entry: {{ ...componentEntries('src/ds/components', 'components') }} }} }} }});\nexport default defineConfig(({{ mode }}) => (process.env.STORYBOOK === 'true' ? {{}} : lib(mode === 'production')));"
    );
    let results = run(&[("vite.config.ts", &conditional)], &["vite"], &[]);
    assert!(output(&results, "vite").entries_incomplete);
}

#[test]
fn each_unreadable_config_is_named_with_its_cause() {
    let cases = [
        (
            "vite.config.ts",
            "export default makeConfig();",
            IncompleteCause::UnknownValue,
        ),
        (
            "vite.config.ts",
            "import base from './base';\nexport default { ...base };",
            IncompleteCause::UnknownSpread,
        ),
        ("vite.config.ts", "export default {", IncompleteCause::Unparsable),
        (
            "vite.config.ts",
            "export default { build: { lib: { entry: pick() } } };",
            IncompleteCause::ComputedEntries,
        ),
    ];
    for (file, source, cause) in cases {
        let results = run(&[(file, source)], &["vite"], &[]);
        let out = output(&results, "vite");
        assert!(out.entries_incomplete, "{source}");
        let reason = out
            .incomplete_reasons
            .iter()
            .find(|reason| reason.cause == cause)
            .unwrap_or_else(|| panic!("{source}: {:?}", out.incomplete_reasons));
        assert_eq!(reason.plugin, "vite");
        assert_eq!(reason.file, Path::new(DIR).join(file));
    }
    let complete = run(&[("vite.config.ts", "export default { base: '/' };")], &["vite"], &[]);
    assert_eq!(output(&complete, "vite").incomplete_reasons, []);
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
        let dependency = if plugin == "playwright" {
            "@playwright/test"
        } else {
            plugin
        };
        let results = run(&[(file, source)], &[dependency], &[]);
        assert!(!output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

/// An esbuild config is a script that runs builds and exports nothing: its `entryPoints` are read wherever it writes
/// them, at the top level through a shared helper or inside a function (ADR 0081).
#[test]
fn esbuild_build_scripts_name_their_entry_points() {
    let helper = "import * as path from 'node:path';\nimport { run } from '../build-common.mts';\n\
                  const srcDir = path.join(import.meta.dirname, 'src');\n\
                  run({ platform: 'node', entryPoints: { main: path.join(srcDir, 'main.ts') }, srcDir }, process.argv);\n";
    let in_function = "const esbuild = require('esbuild');\nconst path = require('path');\n\
                       async function build() {\n  const shared = { entryPoints: [path.join(__dirname, 'src/runtime/index.ts')] };\n\
                       await esbuild.build({ ...shared, format: 'esm' });\n}\nbuild();\n";
    for (file, source, entry) in [
        ("esbuild.mts", helper, "src/main.ts"),
        ("esbuild.config.js", in_function, "src/runtime/index.ts"),
    ] {
        let results = run(&[(file, source)], &["esbuild"], &[]);
        let out = output(&results, "esbuild");
        assert!(!out.entries_incomplete, "{file}: {:?}", out.incomplete_reasons);
        assert_entries(&out.production_entries, &[entry]);
    }

    // Computed entry points leave the entries incomplete; a script that writes none stays unparsable.
    let cases = [
        (
            "import { readdirSync } from 'node:fs';\nimport esbuild from 'esbuild';\n\
             await esbuild.build({ entryPoints: readdirSync('src') });\n",
            IncompleteCause::ComputedEntries,
        ),
        (
            "import esbuild from 'esbuild';\nimport { options } from './options.mjs';\nawait esbuild.build(options);\n",
            IncompleteCause::Unparsable,
        ),
    ];
    for (source, cause) in cases {
        let results = run(&[("esbuild.config.mjs", source)], &["esbuild"], &[]);
        let out = output(&results, "esbuild");
        assert!(out.entries_incomplete, "{source}");
        assert!(
            out.incomplete_reasons.iter().any(|reason| reason.cause == cause),
            "{source}: {:?}",
            out.incomplete_reasons
        );
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
fn playwright_test_match_with_optional_characters_classes_and_lazy_stars() {
    let results = run(
        &[(
            "playwright.config.js",
            r"module.exports = { projects: [
                { testDir: 'e2e', testMatch: /.*\.e2e\.tsx?/ },
                { testDir: 'specs', testMatch: /.*\.(spec|test)\.[jt]sx?$/ },
                { testDir: 'pw', testMatch: /.*?\.pw\.[^d]s$/ },
            ] };",
        )],
        &["@playwright/test"],
        &[],
    );
    let entries = &output(&results, "playwright").entries;
    assert_entries(
        entries,
        &[
            "e2e/**/*.e2e.ts{x,}*",
            "specs/**/*.{spec,test}.[jt]s{x,}",
            "pw/**/*.pw.[!d]s",
        ],
    );
    for dir in ["e2e", "specs", "pw"] {
        assert!(
            !entries.contains(&format!("{dir}/**/*.{{js,jsx,ts,tsx,mjs,cjs,mts,cts}}")),
            "{dir} fell back to every script: {entries:?}"
        );
    }
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
      - run: node $GITHUB_WORKSPACE/scripts/check.js
        working-directory: tools/release
      - run: node ${{ github.workspace }}/scripts/lint.js
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
            "scripts/check.js",
            "./scripts/lint.js",
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

/// A `working-directory` below the workspace the workflow checks out is that directory of the workspace, and the
/// workspace itself is the root. `$GITHUB_ACTION_PATH` is the action's directory only as a whole name.
#[test]
fn github_actions_working_directories_in_the_github_workspace_are_workspace_directories() {
    let workflow = r#"on: [push]
jobs:
  a:
    steps:
      - run: node ./build.js
        working-directory: ${{ github.workspace }}/packages/app
      - run: node ./check.js
        working-directory: ${{github.workspace}}
      - run: node ./outside.js
        working-directory: ${{ github.workspace }}/../elsewhere
      - run: node $GITHUB_ACTION_PATHS/longer.js && node $GITHUB_ACTION_PATH/action.js
"#;
    let results = run(&[(".github/workflows/ci.yml", workflow)], &[], &[]);
    let out = output(&results, "github-actions");
    assert_entries(&out.entries, &["packages/app/build.js", "./check.js", "./action.js"]);
    for skipped in ["outside.js", "longer.js"] {
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
        (
            "vitest",
            "vitest.config.ts",
            "export default { ...shared, test: { include: ['a'] } };",
        ),
        ("jest", "jest.config.js", "module.exports = loadConfig();"),
        (
            "playwright",
            "playwright.config.ts",
            "export default { ...base, testDir: './e2e' };",
        ),
        ("eslint", "eslint.config.js", "export default makeConfig();"),
        ("prettier", ".prettierrc.json", "{ not json at all"),
    ];
    for (plugin, file, source) in cases {
        let dependency = if plugin == "playwright" {
            "@playwright/test"
        } else {
            plugin
        };
        let results = run(&[(file, source)], &[dependency], &[]);
        assert!(output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

#[test]
fn a_readable_config_leaves_the_entries_complete() {
    let cases = [
        (
            "vitest",
            "vitest.config.ts",
            "export default { test: { include: ['a'] } };",
        ),
        (
            "jest",
            "jest.config.js",
            "module.exports = { testMatch: ['**/*.t.js'] };",
        ),
        (
            "playwright",
            "playwright.config.ts",
            "export default { testDir: './e2e' };",
        ),
    ];
    for (plugin, file, source) in cases {
        let dependency = if plugin == "playwright" {
            "@playwright/test"
        } else {
            plugin
        };
        let results = run(&[(file, source)], &[dependency], &[]);
        assert!(!output(&results, plugin).entries_incomplete, "{plugin}: {source}");
    }
}

#[test]
fn a_broken_config_of_a_plugin_reading_the_source_leaves_the_entries_complete() {
    let cases = [
        (
            "sst",
            "sst",
            "sst.config.ts",
            "export default $config({ async run() { new sst.aws.Function(\"A\", {",
        ),
        (
            "stencil",
            "@stencil/core",
            "stencil.config.ts",
            "export const config = { namespace: 'x', outputTargets: [ ;",
        ),
    ];
    for (plugin, dependency, file, source) in cases {
        let results = run(&[(file, source)], &[dependency], &[]);
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

/// A spread of a conditional object literal (`...(tmp && { cacheDir })`, `...(ci ? { … } : {})`) that sets no key
/// the plugin reads entries from leaves the entries complete, as does a ternary on a key that names no file.
#[test]
fn a_conditional_spread_of_object_literals_is_complete() {
    let sources = [
        "import { defineConfig } from 'vitest/config';\n\
         const tmp = process.env.TEST_TMPDIR;\n\
         export default defineConfig({\n\
           ...(tmp && { cacheDir: `${tmp}/vite` }),\n\
           test: { include: ['**/*.test.ts'], reporters: process.env.CI ? ['default', 'junit'] : ['default'] },\n\
         });",
        "import { defineConfig } from 'vitest/config';\n\
         const tmp = process.env.TEST_TMPDIR;\n\
         export default defineConfig({\n\
           ...(tmp ? { cacheDir: `${tmp}/vite` } : {}),\n\
           test: { include: ['**/*.test.ts'], reporters: process.env.CI ? ['default', 'junit'] : ['default'] },\n\
         });",
    ];
    for source in sources {
        let results = run(&[("vitest.config.ts", source)], &["vitest"], &[]);
        let out = output(&results, "vitest");
        assert_eq!(out.incomplete_reasons, [], "{source}");
        assert!(!out.entries_incomplete, "{source}");
        assert_entries(&out.entries, &["**/*.test.ts"]);
    }
}

/// The entry keys of every branch of a conditional spread are credited; a branch that is not known leaves the entries
/// incomplete for an unknown spread.
#[test]
fn a_conditional_spread_credits_the_entries_of_every_branch() {
    let source = "export default {\n\
                    test: { include: ['src/**/*.test.ts'] },\n\
                    ...(process.env.INTEGRATION\n\
                      ? { test: { include: ['it/**/*.it.ts'], setupFiles: ['./it/setup.ts'] } }\n\
                      : { test: { include: ['unit/**/*.test.ts'], globalSetup: ['./unit/global.ts'] } }),\n\
                    ...(process.env.DB && { test: { setupFiles: ['./db/setup.ts'] } }),\n\
                  };";
    let results = run(&[("vitest.config.ts", source)], &["vitest"], &[]);
    let out = output(&results, "vitest");
    assert_eq!(out.incomplete_reasons, []);
    assert_entries(
        &out.entries,
        &[
            "it/**/*.it.ts",
            "unit/**/*.test.ts",
            "it/setup.ts",
            "unit/global.ts",
            "db/setup.ts",
        ],
    );

    let unknown = "import base from './base';\n\
                   export default { ...(process.env.CI ? base : { test: { include: ['a/**/*.test.ts'] } }) };";
    let results = run(&[("vitest.config.ts", unknown)], &["vitest"], &[]);
    let out = output(&results, "vitest");
    assert!(out.entries_incomplete);
    assert!(
        out.incomplete_reasons
            .iter()
            .any(|reason| reason.cause == IncompleteCause::UnknownSpread),
        "{:?}",
        out.incomplete_reasons
    );
}

#[test]
fn regex_alias_finds_become_prefix_aliases_or_computed_ones() {
    let config = r"
export default {
  resolve: {
    alias: [
      { find: /^@\/(.*)$/, replacement: '/src/$1' },
      { find: /^~lib\/(.+)$/, replacement: './lib/$1' },
      { find: /^@gen\/(a|b)$/, replacement: './gen/$1' },
      { find: /(.*)\.js$/, replacement: '$1' },
    ],
  },
};";
    let results = run(&[("vite.config.ts", config)], &["vite"], &[]);
    let aliases: Vec<(String, Option<String>)> = output(&results, "vite")
        .aliases
        .iter()
        .map(|alias| (alias.find.clone(), alias.target.clone()))
        .collect();
    assert_eq!(
        aliases,
        [
            ("@".to_owned(), Some("src".to_owned())),
            ("~lib".to_owned(), Some("lib".to_owned())),
            ("@gen".to_owned(), None),
        ]
    );
}

#[test]
fn jest_module_name_mapper_prefixes_are_aliases() {
    let config = "module.exports = { moduleNameMapper: { '^@core/(.*)$': '<rootDir>/src/$1', '\\\\.css$': 'identity-obj-proxy' } };";
    let results = run(&[("jest.config.js", config)], &["jest"], &[]);
    let aliases: Vec<(String, Option<String>)> = output(&results, "jest")
        .aliases
        .iter()
        .map(|alias| (alias.find.clone(), alias.target.clone()))
        .collect();
    assert_eq!(aliases, [("@core".to_owned(), Some("src".to_owned()))]);
}

/// A key a plugin reads files from that the config computes (`setupFiles: isPostgres ? [...] : []`) is not read as
/// empty: the entries are incomplete. Other unknown options leave them complete.
#[test]
fn an_unknown_entry_key_below_the_top_level_is_incomplete() {
    let cases = [
        (
            "vitest.config.ts",
            "vitest",
            "export default { test: { setupFiles: isPostgres ? ['./pg.ts'] : [] } };",
        ),
        (
            "vitest.config.ts",
            "vitest",
            "export default { test: { include: ['a.test.ts'], ...shared } };",
        ),
        ("vitest.config.ts", "vitest", "export default { test: env() };"),
        (
            "vitest.config.ts",
            "vitest",
            "export default { test: { projects: [base, './pkg'] } };",
        ),
        (
            "jest.config.js",
            "jest",
            "module.exports = { setupFilesAfterEnv: [setup()], testMatch: ['**/*.test.js'] };",
        ),
        ("jest.config.js", "jest", "module.exports = { projects: projects() };"),
    ];
    for (file, plugin, source) in cases {
        let results = run(&[(file, source)], &[plugin], &[]);
        assert!(output(&results, plugin).entries_incomplete, "{source}");
    }
    let known = [
        (
            "vitest.config.ts",
            "vitest",
            "export default { test: { setupFiles: ['./a.ts'], retry: env() } };",
        ),
        (
            "jest.config.js",
            "jest",
            "module.exports = { projects: [{ displayName: name(), testMatch: ['a.test.js'] }] };",
        ),
    ];
    for (file, plugin, source) in known {
        let results = run(&[(file, source)], &[plugin], &[]);
        assert!(!output(&results, plugin).entries_incomplete, "{source}");
    }
}

/// Whether every reason of `plugin` leaves only files matching globs unjudged, and one of them is `glob`.
fn scoped_to(results: &[(PluginId, PluginOutput)], plugin: &str, glob: &str) -> bool {
    let reasons = &output(results, plugin).incomplete_reasons;
    !reasons.is_empty()
        && reasons
            .iter()
            .all(|reason| matches!(&reason.scope, IncompleteScope::Within(globs) if globs.iter().any(|g| g == glob)))
}

fn package_wide(results: &[(PluginId, PluginOutput)], plugin: &str) -> bool {
    let reasons = &output(results, plugin).incomplete_reasons;
    !reasons.is_empty() && reasons.iter().any(|reason| reason.scope == IncompleteScope::Package)
}

/// A config a plugin cannot fully read hides only what that tool takes entries from (ADR 0065).
#[test]
fn incomplete_configs_are_scoped_to_what_the_tool_could_make_an_entry() {
    let spread = "import base from './base';\nexport default { ...base };";
    let vite = run(&[("vite.config.ts", spread)], &["vite"], &[]);
    assert!(scoped_to(
        &vite,
        "vite",
        "{,src/}{index,main}.{js,jsx,ts,tsx,mjs,cjs,mts,cts,vue,svelte,astro,mdx}"
    ));

    // An imported helper or a constant holding an imported value is a shared base too, above the entry key.
    for source in [
        "import { makeLib } from './lib';\nexport default { build: { ...makeLib() } };",
        "import shared from './shared';\nconst lib = shared.lib;\nexport default { build: { lib: { ...lib } } };",
    ] {
        let vite = run(&[("vite.config.ts", source)], &["vite"], &[]);
        assert!(
            scoped_to(
                &vite,
                "vite",
                "{,src/}{index,main}.{js,jsx,ts,tsx,mjs,cjs,mts,cts,vue,svelte,astro,mdx}"
            ),
            "{source}: {:?}",
            output(&vite, "vite").incomplete_reasons
        );
    }

    let vitest = run(&[("vitest.config.ts", spread)], &["vitest"], &[]);
    assert!(scoped_to(
        &vitest,
        "vitest",
        "**/*{setup,Setup,teardown,Teardown}*.{js,jsx,ts,tsx,mjs,cjs,mts,cts,vue,svelte,astro,mdx}"
    ));
    // A test config merged into an unknown base is still a test config.
    let merged = "import { mergeConfig } from 'vitest/config';\nimport base from './base';\n\
                  export default mergeConfig(base, { test: { environment: 'node' } });";
    let merged = run(&[("vitest.config.ts", merged)], &["vitest"], &[]);
    assert!(scoped_to(
        &merged,
        "vitest",
        "**/__mocks__/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}"
    ));

    let next = run(
        &[("next.config.js", "module.exports = (phase) => wrap({});")],
        &["next"],
        &[],
    );
    assert!(scoped_to(&next, "next", "{,src/}{app,pages}/**"));

    let playwright = run(&[("playwright.config.ts", spread)], &["@playwright/test"], &[]);
    assert!(scoped_to(
        &playwright,
        "playwright",
        "**/{test,tests,__tests__,__mocks__,__fixtures__,e2e,spec,specs,fixtures,mocks,testing,test-utils}/**"
    ));
    // A config in a subdirectory scopes its conventions to that directory.
    let nested = run(
        &[("e2e/playwright.config.ts", spread)],
        &["@playwright/test"],
        &[("e2e", "playwright test -c e2e/playwright.config.ts")],
    );
    assert!(scoped_to(
        &nested,
        "playwright",
        "e2e/**/{test,tests,__tests__,__mocks__,__fixtures__,e2e,spec,specs,fixtures,mocks,testing,test-utils}/**"
    ));
}

/// A key naming files the config computes itself (a spread inside it or above it of a value the config computes
/// included), an unknown Vite or Vitest config, a config that does not parse and a plugin that gives no scope keep the
/// whole package unjudged.
#[test]
fn computed_unparsable_and_unscoped_configs_stay_package_wide() {
    let cases: [(&str, &str, &str); 15] = [
        (
            "vite",
            "vite.config.ts",
            "export default { build: { lib: { entry: pick() } } };",
        ),
        // A spread above the entry key of a value the config computes itself, not of an imported base.
        (
            "vite",
            "vite.config.ts",
            "const makeLib = () => JSON.parse(process.env.LIB ?? '{}');\nexport default { build: { ...makeLib() } };",
        ),
        (
            "vite",
            "vite.config.ts",
            "function libEntries() {\n  return JSON.parse(process.env.ENTRIES ?? '{}');\n}\n\
             export default { build: { lib: { ...libEntries() } } };",
        ),
        (
            "vitest",
            "vitest.config.ts",
            "const local = JSON.parse(process.env.TEST ?? '{}');\nexport default { test: { ...local } };",
        ),
        (
            "vite",
            "vite.config.ts",
            "export default { build: { lib: { entry: { index: 'src/index.ts', ...pick() } } } };",
        ),
        (
            "vite",
            "vite.config.ts",
            "import base from './base';\nexport default { ...base, build: { rollupOptions: { input: pick() } } };",
        ),
        ("vite", "vite.config.ts", "export default makeConfig();"),
        ("vite", "vite.config.ts", "export default {"),
        (
            "vitest",
            "vitest.config.ts",
            "export default { root: here(), test: {} };",
        ),
        (
            "vitest",
            "vitest.config.ts",
            "export default { test: { include: process.env.CI ? ['src/**/*.check.ts'] : ['src/**/*.unit.ts'] } };",
        ),
        (
            "vitest",
            "vitest.config.ts",
            "import base from './base';\nexport default { ...base, test: { setupFiles: [pick()] } };",
        ),
        (
            "vitest",
            "vitest.config.ts",
            "export default { test: { projects: [{ test: { globalSetup: pick() } }] } };",
        ),
        // A factory called with the package's options may name any of its files.
        (
            "vitest",
            "vitest.config.ts",
            "export default make({ include: ['src/**/*.check.ts'] });",
        ),
        (
            "playwright",
            "playwright.config.ts",
            "export default make({ testDir: './browser', testMatch: '**/*.pw.ts' });",
        ),
        ("webpack", "webpack.config.js", "module.exports = { entry: pick() };"),
    ];
    for (plugin, file, source) in cases {
        let dependency = if plugin == "playwright" {
            "@playwright/test"
        } else {
            plugin
        };
        let results = run(&[(file, source)], &[dependency], &[]);
        assert!(
            package_wide(&results, plugin),
            "{plugin}: {source}: {:?}",
            output(&results, plugin).incomplete_reasons
        );
    }
}
