//! knip's configuration hints reported by `stale-js-config` beyond empty patterns: `entry-redundant`,
//! `project-redundant`, `entry-top-level`, `project-top-level`, `project-extension-*`, `package-entry` and the
//! `*-unconfigured` ratio hints.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const MONOREPO: (&str, &str) = (
    "package.json",
    r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
);

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    lint_at(ROOT, ROOT, files, toml)
}

/// Lints the workspace at `root` of the `files`, whose paths are relative to `base`.
fn lint_at(base: &str, root: &str, files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(base).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(root), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = PluginSource::new(config.js.plugins.clone())
        .with_workspaces(&config.js.workspaces)
        .with_overrides(&config.overrides);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `path: message` of the diagnostics of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| format!("{}: {}", d.location.path.display(), d.message))
        .collect();
    found.sort();
    found
}

fn hints(diagnostics: &[Diagnostic]) -> String {
    found(diagnostics, "stale-js-config").join("\n")
}

#[test]
fn redundant_entry_and_project_patterns() {
    let files = [
        MONOREPO,
        (
            "packages/a/package.json",
            r#"{"name":"a","main":"./src/index.ts","scripts":{"gen":"node scripts/gen.ts"}}"#,
        ),
        ("packages/a/src/index.ts", "import \"./other\";\n"),
        ("packages/a/src/other.ts", "export {};\n"),
        ("packages/a/scripts/gen.ts", "export {};\n"),
    ];
    let diagnostics = lint(
        &files,
        "[js.workspaces.\"packages/a\"]\nentry = [\"src/index.ts\", \"./scripts/gen.ts\", \"src/other.ts\", \"src/*.ts\"]\n\
         project = [\"src/index.ts\", \"src/**/*.ts\", \"scripts/**/*.ts\"]\n",
    );
    insta::assert_snapshot!(hints(&diagnostics));
}

#[test]
fn top_level_entry_and_project_in_a_monorepo() {
    let files = [
        MONOREPO,
        ("index.ts", "export {};\n"),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("packages/a/src/index.ts", "export {};\n"),
    ];
    let toml = "[js]\nentry = [\"index.ts\", \"src/index.ts\"]\nproject = [\"**/*.ts\"]\n";
    insta::assert_snapshot!(hints(&lint(&files, toml)));
    // A single package has nowhere else to put them.
    let single = [
        ("package.json", r#"{"name":"app"}"#),
        ("index.ts", "export {};\n"),
        ("src/index.ts", "export {};\n"),
    ];
    assert_eq!(hints(&lint(&single, toml)), "");
}

#[test]
fn project_extensions_skif_does_not_analyse_or_leaves_out() {
    let files = [
        ("package.json", r#"{"name":"app","dependencies":{"vue":"3"}}"#),
        ("src/index.ts", "import App from \"./App.vue\";\nconsole.log(App);\n"),
        ("src/App.vue", "<script setup lang=\"ts\">\n</script>\n"),
        ("src/data.json", "{}\n"),
    ];
    let diagnostics = lint(&files, "[js]\nproject = [\"src/**/*.{ts,json}\", \"!src/**/*.d.ts\"]\n");
    insta::assert_snapshot!(hints(&diagnostics));
    // Patterns that name no extension, or every component extension the package has, leave nothing out.
    let quiet = lint(&files, "[js]\nproject = [\"src/**\", \"src/**/*.{ts,vue}\"]\n");
    assert_eq!(hints(&quiet), "");
}

#[test]
fn stylesheet_extensions_in_project_are_read_and_unknown_ones_are_not() {
    let files = [
        ("package.json", r#"{"name":"app"}"#),
        ("src/index.ts", "import \"./app.css\";\nimport \"./theme.scss\";\n"),
        ("src/app.css", "@import \"./base.less\";\n"),
        ("src/base.less", "a { color: red; }\n"),
        ("src/theme.scss", "a { color: blue; }\n"),
        ("src/data.foo", "x\n"),
    ];
    // A migrated knip configuration that adds stylesheets to the project: skif reads them, so nothing is stale.
    let migrated = lint(
        &files,
        "[js]\nentry = [\"src/index.ts!\"]\nproject = [\"src/**/*.{ts,css}!\"]\n",
    );
    assert_eq!(hints(&migrated), "");
    let stylesheets = lint(
        &files,
        "[js]\nproject = [\"src/**/*.{ts,css,scss,sass,less,pcss,styl,stylus}\"]\n",
    );
    assert_eq!(hints(&stylesheets), "");
    // An extension skif does not read is still reported, also next to stylesheets in the same pattern.
    let unknown = lint(&files, "[js]\nproject = [\"src/**/*.{ts,css,foo}!\"]\n");
    assert_eq!(
        hints(&unknown),
        "package.json: `.foo` files selected by `js.project` are not analysed"
    );
}

#[test]
fn published_paths_that_name_no_file() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","main":"index.js","types":"./types.d.ts","bin":{"app":"./cli.js"},
               "exports":{".":"./index.js","./gen":"./generated/x.js","./built":"./dist/x.js","./style.css":"./dist/style.css","./wasm":"./build/x.wasm","./src":"./src/*.js","./lib":"./lib/x.js","./run":"./bin/run.js"},
               "dependencies":{"other":"1"},"module":"other/esm.js"}"#,
        ),
        (".gitignore", "generated/\n"),
        ("cli.js", "#!/usr/bin/env node\n"),
        ("src/a.ts", "export {};\n"),
        ("lib/x.ts", "export {};\n"),
    ];
    insta::assert_snapshot!(hints(&lint(&files, "")));
}

/// A package whose `bin` names a file in no `bin` directory with no source behind it, and whose script runs the real
/// code.
const DANGLING_BIN: (&str, &str) = (
    "package.json",
    r#"{"name":"r2b","private":true,"bin":{"r2b":"bin/r2b.js"},"scripts":{"start":"node app/main.js"},
       "dependencies":{"left-pad":"1.3.0"}}"#,
);

/// The same package publishing build output instead.
const BUILT_MAIN: (&str, &str) = (
    "package.json",
    r#"{"name":"r2b","private":true,"main":"./dist/index.js","scripts":{"start":"node app/main.js"},
       "dependencies":{"left-pad":"1.3.0"}}"#,
);

const APP_MAIN: (&str, &str) = ("app/main.js", "console.log('r2b');\n");

const LEFT_PAD_UNUSED: &str = "package.json: `left-pad` is listed in `dependencies` but never used";

/// A path into nothing is reported, contributes no entry and leaves the package judged from its other entries
/// (ADR 0069).
#[test]
fn a_bin_path_into_nothing_is_reported_and_the_package_is_judged() {
    let diagnostics = lint(&[DANGLING_BIN, APP_MAIN], "");
    assert_eq!(
        hints(&diagnostics),
        "package.json: `bin/r2b.js` in `bin` of `package.json` names no file"
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), [LEFT_PAD_UNUSED]);
}

/// A bin path a `.gitignore` hides is build output not built yet: not reported, and the package stays unjudged.
#[test]
fn a_gitignored_bin_path_keeps_the_package_unjudged() {
    let diagnostics = lint(&[DANGLING_BIN, APP_MAIN, (".gitignore", "bin/\n")], "");
    assert_eq!(hints(&diagnostics), "");
    assert_eq!(found(&diagnostics, "unused-dependencies"), Vec::<String>::new());
}

/// A bin path git ignores through the repository's `info/exclude` is build output not built yet.
#[test]
fn a_bin_path_excluded_by_the_repository_keeps_the_package_unjudged() {
    let diagnostics = lint(&[DANGLING_BIN, APP_MAIN, (".git/info/exclude", "bin/\n")], "");
    assert_eq!(hints(&diagnostics), "");
    assert_eq!(found(&diagnostics, "unused-dependencies"), Vec::<String>::new());
}

/// A `.gitignore` between the repository root and the analysed root hides the bin path too.
#[test]
fn a_bin_path_gitignored_above_the_analysed_root_keeps_the_package_unjudged() {
    let files = [
        (".git/HEAD", "ref: refs/heads/main\n"),
        (".gitignore", "app/bin/\n"),
        ("app/package.json", DANGLING_BIN.1),
        ("app/app/main.js", APP_MAIN.1),
    ];
    let diagnostics = lint_at("/repo", "/repo/app", &files, "");
    assert_eq!(hints(&diagnostics), "");
    assert_eq!(found(&diagnostics, "unused-dependencies"), Vec::<String>::new());
    // Without the `.gitignore` the same path is reported, so the work tree above the root is what hides it.
    let diagnostics = lint_at("/repo", "/repo/app", &[files[0], files[2], files[3]], "");
    assert_eq!(
        hints(&diagnostics),
        "package.json: `bin/r2b.js` in `bin` of `package.json` names no file"
    );
}

/// The `src/cli.ts` a build step bundles into `bin`, with no `bin` directory yet.
const CLI_SOURCE: (&str, &str) = ("src/cli.ts", "console.log('cli');\n");

/// A bin path a build step of the package may write is build output not built yet: a build script, a script that
/// runs a bundler, a bundler configuration a plugin reads, or a `tsconfig` whose `outDir` is `bin`. Not reported, and
/// the package stays unjudged.
#[test]
fn a_bin_path_a_build_step_may_write_keeps_the_package_unjudged() {
    let manifest = |extra: &str| {
        format!(
            r#"{{"name":"r2b","private":true,"bin":{{"r2b":"bin/r2b.js"}},"dependencies":{{"left-pad":"1.3.0"}},{extra}}}"#
        )
    };
    let build_script = manifest(r#""scripts":{"start":"node app/main.js","build":"node scripts/build.js"}"#);
    let bundler_script = manifest(
        r#""scripts":{"start":"node app/main.js","bundle":"esbuild src/cli.ts --bundle --outfile=bin/r2b.js"},
           "devDependencies":{"esbuild":"0.25.0"}"#,
    );
    let tsup = manifest(r#""scripts":{"start":"node app/main.js"},"devDependencies":{"tsup":"8.0.0"}"#);
    let tsc = manifest(r#""scripts":{"start":"node app/main.js"},"devDependencies":{"typescript":"5.0.0"}"#);
    let cases: [(&str, Vec<(&str, &str)>); 4] = [
        (
            "build script",
            vec![
                ("package.json", build_script.as_str()),
                ("scripts/build.js", "console.log('build');\n"),
            ],
        ),
        ("bundler script", vec![("package.json", bundler_script.as_str())]),
        (
            "tsup config",
            vec![
                ("package.json", tsup.as_str()),
                ("tsup.config.ts", "export default { entry: ['src/cli.ts'] };\n"),
            ],
        ),
        (
            "tsconfig outDir",
            vec![
                ("package.json", tsc.as_str()),
                (
                    "tsconfig.json",
                    r#"{"compilerOptions":{"outDir":"bin","rootDir":"cli"}}"#,
                ),
            ],
        ),
    ];
    for (case, mut files) in cases {
        files.extend([APP_MAIN, CLI_SOURCE]);
        let diagnostics = lint(&files, "");
        assert_eq!(hints(&diagnostics), "", "{case}");
        assert_eq!(
            found(&diagnostics, "unused-dependencies"),
            Vec::<String>::new(),
            "{case}"
        );
        assert_eq!(found(&diagnostics, "unused-files"), Vec::<String>::new(), "{case}");
    }
}

/// A script not named as a build that compiles with a subcommand (`bun build`, `vite build`, `deno compile`) or names
/// a path in `bin` as its output may write the bin path too.
#[test]
fn a_bin_path_a_compiling_command_may_write_keeps_the_package_unjudged() {
    for script in [
        "bun build src/cli.ts --outfile bin/r2b.js",
        "vite build",
        "deno compile -A src/cli.ts",
        "pnpm exec parcel build src/cli.ts",
        "my-bundler src/cli.ts -o ./bin/r2b.js",
        "my-bundler src/cli.ts --out-dir=bin",
    ] {
        let manifest = format!(
            r#"{{"name":"r2b","private":true,"bin":{{"r2b":"bin/r2b.js"}},"scripts":{{"start":"node app/main.js","compile":"{script}"}},"dependencies":{{"left-pad":"1.3.0"}}}}"#
        );
        let diagnostics = lint(&[("package.json", manifest.as_str()), APP_MAIN, CLI_SOURCE], "");
        assert_eq!(hints(&diagnostics), "", "{script}");
        assert_eq!(
            found(&diagnostics, "unused-dependencies"),
            Vec::<String>::new(),
            "{script}"
        );
    }
    // A command writing elsewhere leaves the path into nothing reported.
    let manifest = r#"{"name":"r2b","private":true,"bin":{"r2b":"bin/r2b.js"},"scripts":{"start":"node app/main.js","compile":"my-bundler src/cli.ts -o out/r2b.js"},"dependencies":{"left-pad":"1.3.0"}}"#;
    let diagnostics = lint(&[("package.json", manifest), APP_MAIN, CLI_SOURCE], "");
    assert_eq!(
        hints(&diagnostics),
        "package.json: `bin/r2b.js` in `bin` of `package.json` names no file"
    );
}

/// A CDK app whose template-generated `bin` names a file nothing provides, with `scripts` completing the manifest:
/// `cdk.json` runs the real entry, `app/main.ts`.
fn cdk_app(scripts: &str) -> String {
    format!(
        r#"{{"name":"@acme/stack","version":"0.1.0","bin":{{"stack":"bin/stack.js"}},
           "scripts":{{"cdk":"cdk",{scripts}}},"dependencies":{{"left-pad":"1.3.0","spare":"1.0.0"}},
           "devDependencies":{{"aws-cdk":"2.0.0","ts-node":"10.9.2","typescript":"5.0.0"}}}}"#
    )
}

/// The files of [`cdk_app`] besides its manifest and `tsconfig.json`.
const CDK_APP_FILES: [(&str, &str); 2] = [
    ("cdk.json", r#"{ "app": "pnpm exec ts-node --prefer-ts-exts app/main.ts" }"#),
    ("app/main.ts", "import leftPad from 'left-pad';\nleftPad('a', 2);\n"),
];

/// The `tsconfig.json` of [`cdk_app`]: no build directory, so the compiler writes each output next to its source.
const CDK_TSCONFIG: (&str, &str) = (
    "tsconfig.json",
    r#"{ "compilerOptions": { "module": "node16", "moduleResolution": "node16" } }"#,
);

/// A `bin` path the TypeScript compiler cannot write, since no file of the package has its name, is a path into
/// nothing although a build script runs the compiler: reported, and the package is judged from the entry the
/// `aws-cdk` plugin reads.
#[test]
fn a_bin_path_the_typescript_compiler_cannot_write_is_reported_and_the_package_is_judged() {
    for build in [
        r#""build":"tsc""#,
        r#""build":"tsc -p tsconfig.build.json && tsc -p tsconfig.esm.json""#,
        r#""build":"tsgo","watch":"tsc -w""#,
    ] {
        let manifest = cdk_app(build);
        let mut files = vec![("package.json", manifest.as_str()), CDK_TSCONFIG];
        files.extend(CDK_APP_FILES);
        let diagnostics = lint(&files, "");
        assert_eq!(
            hints(&diagnostics),
            "package.json: `bin/stack.js` in `bin` of `package.json` names no file",
            "{build}"
        );
        assert_eq!(
            found(&diagnostics, "unused-dependencies"),
            ["package.json: `spare` is listed in `dependencies` but never used"],
            "{build}"
        );
    }
}

/// A `bin` path the TypeScript compiler may write keeps the package unjudged as before: a source file of its name
/// exists (`src/bin/stack.ts`, compiled into the package directory), the build runs more than the compiler, or the
/// compiler writes one named file (`--outFile`).
#[test]
fn a_bin_path_the_typescript_compiler_may_write_keeps_the_package_unjudged() {
    let tsc = cdk_app(r#""build":"tsc""#);
    let copied = cdk_app(r#""build":"tsc && node scripts/copy-bin.js""#);
    let out_file = cdk_app(r#""build":"tsc --outFile bin/stack.js""#);
    let cases: [(&str, Vec<(&str, &str)>); 3] = [
        (
            "source of that name",
            vec![
                ("package.json", tsc.as_str()),
                (
                    "tsconfig.json",
                    r#"{ "compilerOptions": { "outDir": ".", "rootDir": "src" }, "include": ["src"] }"#,
                ),
                ("src/bin/stack.ts", "console.log('stack');\n"),
            ],
        ),
        (
            "more than the compiler",
            vec![
                ("package.json", copied.as_str()),
                CDK_TSCONFIG,
                ("scripts/copy-bin.js", "console.log('copy');\n"),
            ],
        ),
        (
            "one output file",
            vec![("package.json", out_file.as_str()), CDK_TSCONFIG],
        ),
    ];
    for (case, mut files) in cases {
        files.extend(CDK_APP_FILES);
        let diagnostics = lint(&files, "");
        assert_eq!(hints(&diagnostics), "", "{case}");
        assert_eq!(
            found(&diagnostics, "unused-dependencies"),
            Vec::<String>::new(),
            "{case}"
        );
    }
}

/// Build output is never a path into nothing: with its source present it maps, with neither the source nor the file
/// present (built or not) the package stays unjudged, and so does a bin stub that loads output with no source behind
/// it.
#[test]
fn build_output_is_never_a_path_into_nothing() {
    let mapped = lint(&[BUILT_MAIN, APP_MAIN, ("src/index.ts", "export {};\n")], "");
    assert_eq!(hints(&mapped), "");
    assert_eq!(found(&mapped, "unused-dependencies"), [LEFT_PAD_UNUSED]);

    let unbuilt = lint(&[BUILT_MAIN, APP_MAIN], "");
    assert_eq!(hints(&unbuilt), "");
    assert_eq!(found(&unbuilt, "unused-dependencies"), Vec::<String>::new());

    let built = lint(&[BUILT_MAIN, APP_MAIN, ("dist/other.js", "module.exports = {};\n")], "");
    assert_eq!(hints(&built), "");
    assert_eq!(found(&built, "unused-dependencies"), Vec::<String>::new());

    let stub = [
        (
            "package.json",
            r#"{"name":"r2b","private":true,"bin":{"r2b":"bin/r2b"},"scripts":{"start":"node app/main.js"},
               "dependencies":{"left-pad":"1.3.0"}}"#,
        ),
        ("bin/r2b", "#!/usr/bin/env node\nrequire('../dist/cli.js');\n"),
        APP_MAIN,
    ];
    let stub = lint(&stub, "");
    assert_eq!(hints(&stub), "");
    assert_eq!(found(&stub, "unused-dependencies"), Vec::<String>::new());
}

fn orphans(dir: &str, count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("{dir}orphan{index}.ts"), "export {};\n".to_owned()))
        .collect()
}

#[test]
fn many_unused_files_suggest_missing_entries() {
    let owned = |extra: Vec<(String, String)>, base: &[(&'static str, &'static str)]| {
        let mut files: Vec<(String, String)> = base
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect();
        files.extend(extra);
        files
    };
    let run = |files: &[(String, String)]| {
        let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        hints(&lint(&borrowed, ""))
    };
    let single = [("package.json", r#"{"name":"app"}"#), ("index.ts", "export {};\n")];
    // knip's thresholds: more than 20 unused files, more than a fifth as many as the files reached.
    assert_eq!(run(&owned(orphans("src/", 20), &single)), "");
    let many = run(&owned(orphans("src/", 21), &single));
    // `index.ts` and the files it imports are reached: 21 unused files are a fifth of 105, but more than a fifth of 104.
    let reached = |count: usize| {
        let imports = (1..count)
            .map(|index| format!("import \"./src/used{index}\";\n"))
            .collect::<Vec<_>>()
            .concat();
        let mut files = vec![
            ("package.json".to_owned(), r#"{"name":"app"}"#.to_owned()),
            ("index.ts".to_owned(), imports),
        ];
        files.extend((1..count).map(|index| (format!("src/used{index}.ts"), "export {};\n".to_owned())));
        files.extend(orphans("src/", 21));
        files
    };
    assert_eq!(run(&reached(105)), "");
    assert_eq!(
        run(&reached(104)),
        "package.json: 21 of 125 files are unused, so `[js]` likely misses entry files"
    );
    let monorepo = [
        MONOREPO,
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("packages/a/index.ts", "export {};\n"),
        ("packages/b/package.json", r#"{"name":"b"}"#),
        ("packages/b/index.ts", "export {};\n"),
        ("packages/b/lone.ts", "export {};\n"),
    ];
    let mut extra = orphans("packages/a/src/", 15);
    extra.extend(orphans("packages/b/src/", 7));
    let workspaces = run(&owned(extra, &monorepo));
    insta::assert_snapshot!(format!("{many}\n---\n{workspaces}"));
}
