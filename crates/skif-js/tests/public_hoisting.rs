//! `[js] use-public-hoisting`: a package that pnpm's `public-hoist-pattern` or `shamefully-hoist`, or rules_js's
//! `public_hoist_packages`, hoists into a `node_modules` above a package is available to it without a listing.
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

const ON: &str = "[js]\nuse-public-hoisting = true\n";
const OFF: &str = "";

const PNPM10: &str = r#"{"name":"root","private":true,"packageManager":"pnpm@10.0.0"}"#;
const PNPM11: &str = r#"{"name":"root","private":true,"packageManager":"pnpm@11.0.0"}"#;
const NPM: &str = r#"{"name":"root","private":true,"packageManager":"npm@10.0.0","workspaces":["packages/*"]}"#;
const PACKAGES: &str = "packages:\n  - packages/*\n";

const APP: (&str, &str) = (
    "packages/app/package.json",
    r#"{"name":"app","version":"1.0.0","main":"index.js"}"#,
);
const APP_INDEX: (&str, &str) = (
    "packages/app/index.js",
    "import a from 'hoisted-only';\nimport b from '@scope/tool';\nimport c from 'other';\nexport default [a, b, c];\n",
);

const HOISTED_ONLY: &str = "packages/app/index.js: hoisted-only";
const SCOPE_TOOL: &str = "packages/app/index.js: @scope/tool";
const OTHER: &str = "packages/app/index.js: other";
const ALL: &[&str] = &[SCOPE_TOOL, HOISTED_ONLY, OTHER];

fn lint(files: &[(&str, &str)], toml: &str, strict: bool) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let mut config = Config::from_toml_str(toml).unwrap();
    config.js.isolate_workspaces = strict;
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// `<path>: <dependency>` of every diagnostic of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display(),
                d.dependency.clone().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

/// The unlisted dependencies of `app`, which imports `hoisted-only`, `@scope/tool` and `other` and lists none, in a
/// workspace with the root manifest `root`, the `pnpm-workspace.yaml` `yaml` (none when `None`) and `files`.
fn unlisted(root: &str, yaml: Option<&str>, files: &[(&str, &str)], toml: &str, strict: bool) -> Vec<String> {
    let mut all = vec![("package.json", root), APP, APP_INDEX];
    if let Some(yaml) = yaml {
        all.push(("pnpm-workspace.yaml", yaml));
    }
    all.extend_from_slice(files);
    found(&lint(&all, toml, strict), "unlisted-dependencies")
}

/// The root manifest, `pnpm-workspace.yaml`, the other files, the configuration and the unlisted dependencies.
type Case<'a> = (
    &'a str,
    Option<&'a str>,
    &'a [(&'a str, &'a str)],
    &'a str,
    &'a [&'a str],
);

/// With the setting on, pnpm's public hoist patterns are read as pnpm reads them: `public-hoist-pattern` of the root
/// `.npmrc` before pnpm 11 (array entries accumulate, a later plain entry adds to an array, a plain entry after a
/// plain entry replaces it), else `publicHoistPattern` of `pnpm-workspace.yaml`, which wins over `.npmrc`;
/// `shamefully-hoist` hoists everything (`*`); a `!` pattern excludes what an earlier pattern hoists, and patterns
/// that only exclude hoist everything else. Off, a pnpm 11 `.npmrc`, a commented-out line, one under a section, a
/// member's `.npmrc`, npm, and no setting at all (pnpm's default is not assumed) hoist nothing.
#[test]
fn pnpm_public_hoist_settings_make_packages_available_when_on() {
    let npmrc = |text| [(".npmrc", text)];
    let pattern = "public-hoist-pattern[]=hoisted-only\n";
    let shameful_yaml = "packages:\n  - packages/*\nshamefullyHoist: true\n";
    let hoist_yaml = "packages:\n  - packages/*\npublicHoistPattern:\n  - hoisted-only\n";
    let empty_hoist_yaml = "packages:\n  - packages/*\npublicHoistPattern: []\n";
    let pnpm = Some(PACKAGES);
    let cases: [Case<'_>; 21] = [
        (PNPM10, pnpm, &npmrc(pattern), OFF, ALL),
        (PNPM10, pnpm, &npmrc(pattern), ON, &[SCOPE_TOOL, OTHER]),
        (PNPM10, pnpm, &npmrc("public-hoist-pattern[]=@scope/*\n"), ON, &[HOISTED_ONLY, OTHER]),
        (PNPM10, pnpm, &npmrc("public-hoist-pattern[]=*-only\n"), ON, &[SCOPE_TOOL, OTHER]),
        (
            PNPM10,
            pnpm,
            &npmrc("public-hoist-pattern[]=*\npublic-hoist-pattern[]=!other\n"),
            ON,
            &[OTHER],
        ),
        (PNPM10, pnpm, &npmrc("public-hoist-pattern[]=!other\n"), ON, &[OTHER]),
        (PNPM10, pnpm, &npmrc("public-hoist-pattern=hoisted-only\n"), ON, &[SCOPE_TOOL, OTHER]),
        (
            PNPM10,
            pnpm,
            &npmrc("public-hoist-pattern=other\npublic-hoist-pattern=hoisted-only\n"),
            ON,
            &[SCOPE_TOOL, OTHER],
        ),
        (
            PNPM10,
            pnpm,
            &npmrc("public-hoist-pattern=hoisted-only\npublic-hoist-pattern[]=other\n"),
            ON,
            &[SCOPE_TOOL],
        ),
        (PNPM10, pnpm, &npmrc("shamefully-hoist=true\n"), ON, &[]),
        (PNPM10, pnpm, &npmrc("shamefully-hoist=false\n"), ON, ALL),
        (PNPM10, pnpm, &npmrc("# public-hoist-pattern[]=hoisted-only\n"), ON, ALL),
        (PNPM10, pnpm, &npmrc("[section]\npublic-hoist-pattern[]=hoisted-only\n"), ON, ALL),
        (PNPM10, pnpm, &[("packages/app/.npmrc", pattern)], ON, ALL),
        (PNPM10, pnpm, &[], ON, ALL),
        (PNPM10, Some(hoist_yaml), &[], ON, &[SCOPE_TOOL, OTHER]),
        (PNPM10, Some(empty_hoist_yaml), &npmrc(pattern), ON, ALL),
        (PNPM10, Some(shameful_yaml), &[], ON, &[]),
        (PNPM11, pnpm, &npmrc(pattern), ON, ALL),
        (PNPM11, Some(hoist_yaml), &[], ON, &[SCOPE_TOOL, OTHER]),
        (NPM, None, &npmrc(pattern), ON, ALL),
    ];
    for (root, yaml, files, toml, expected) in cases {
        assert_eq!(
            unlisted(root, yaml, files, toml, false),
            expected,
            "{root} {yaml:?} {files:?} {toml:?}"
        );
    }
}

/// With the setting on, `public_hoist_packages` of rules_js's `npm_translate_lock`, in the Bazel repository's
/// `MODULE.bazel`, `WORKSPACE.bazel` or `WORKSPACE`, hoists each key (a version suffix dropped) into the
/// `node_modules` of each Bazel package it lists, `""` being the repository root: it is available to the packages at
/// or below that directory. A commented-out call and the setting off hoist nothing.
#[test]
fn bazel_public_hoist_packages_make_packages_available_where_they_are_hoisted() {
    let module = |text| [("MODULE.bazel", text)];
    let root_hoist = "npm = use_extension(\"@aspect_rules_js//npm:extensions.bzl\", \"npm\")\n\
        npm.npm_translate_lock(\n    name = \"npm\",\n    pnpm_lock = \"//:pnpm-lock.yaml\",\n    \
        public_hoist_packages = {\"hoisted-only\": [\"\"]},\n)\n";
    let per_package = "npm.npm_translate_lock(name = \"npm\", public_hoist_packages = {\n    \
        \"@scope/tool@1.2.3\": [\"packages/app\"],\n    \"other\": [\"packages/web\"],\n})\n";
    let workspace_file = [(
        "WORKSPACE",
        "npm_translate_lock(name = \"npm\", public_hoist_packages = {\"hoisted-only\": [\"\"]})\n",
    )];
    let commented = "# npm.npm_translate_lock(name = \"npm\", public_hoist_packages = {\"hoisted-only\": [\"\"]})\n";
    let pnpm = Some(PACKAGES);
    let cases: [Case<'_>; 5] = [
        (PNPM10, pnpm, &module(root_hoist), OFF, ALL),
        (PNPM10, pnpm, &module(root_hoist), ON, &[SCOPE_TOOL, OTHER]),
        (PNPM10, pnpm, &module(per_package), ON, &[HOISTED_ONLY, OTHER]),
        (PNPM10, pnpm, &workspace_file, ON, &[SCOPE_TOOL, OTHER]),
        (PNPM10, pnpm, &module(commented), ON, ALL),
    ];
    for (root, yaml, files, toml, expected) in cases {
        assert_eq!(
            unlisted(root, yaml, files, toml, false),
            expected,
            "{files:?} {toml:?}"
        );
    }
}

/// knip's strict mode takes only a package's own manifest, so a hoisted package is unlisted there with the setting on.
#[test]
fn strict_mode_does_not_count_hoisted_packages() {
    let npmrc = [(".npmrc", "shamefully-hoist=true\n")];
    assert_eq!(unlisted(PNPM10, Some(PACKAGES), &npmrc, ON, false), Vec::<String>::new());
    assert_eq!(unlisted(PNPM10, Some(PACKAGES), &npmrc, ON, true), ALL);
}

/// Hoisting makes nothing used: a listed dependency nothing imports stays unused with everything hoisted, and an
/// imported listed one stays used.
#[test]
fn hoisting_leaves_unused_dependencies_as_they_are() {
    let files = [
        ("package.json", PNPM10),
        ("pnpm-workspace.yaml", PACKAGES),
        (".npmrc", "shamefully-hoist=true\n"),
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"index.js","dependencies":{"hoisted-only":"1.0.0","idle":"1.0.0"}}"#,
        ),
        ("packages/app/index.js", "import a from 'hoisted-only';\nexport default a;\n"),
    ];
    for toml in [OFF, ON] {
        let diagnostics = lint(&files, toml, false);
        assert_eq!(
            found(&diagnostics, "unused-dependencies"),
            ["packages/app/package.json: idle"],
            "{toml:?}"
        );
        assert_eq!(found(&diagnostics, "unlisted-dependencies"), Vec::<String>::new(), "{toml:?}");
    }
}
