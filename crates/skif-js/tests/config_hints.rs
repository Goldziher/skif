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
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
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
fn published_paths_that_name_no_file() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","main":"index.js","types":"./types.d.ts","bin":{"app":"./cli.js"},
               "exports":{".":"./index.js","./gen":"./generated/x.js","./built":"./dist/x.js","./style.css":"./dist/style.css","./wasm":"./build/x.wasm","./src":"./src/*.js","./lib":"./lib/x.js"},
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
