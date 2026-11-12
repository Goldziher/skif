//! Findings parity with knip: runs skif and a pinned knip on each corpus repository and diffs the findings.
//!
//! Both tools' findings are reduced to a key per finding, `<file>|<name>` within a knip issue type. The two sides
//! disagree where the tools model things differently, so the numbers are a measure of drift, not a verdict: the
//! committed baselines say what the agreement was, and `--check` fails when it moves.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use skif_js::{IncompleteScopeGlobs, is_out_dir, n8n_load_paths, source_candidates};
use skif_js_syntax::{ExportKind, Import, ImportKind, ImportedKind, ModuleInfo, ParseOptions, parse_module};
use skif_workspace::{OsFs, declared_member_dirs};

use crate::corpus::{self, RUSH_KNIP_ROOT, Repo, SCRATCH_ROOT_MARK, Tools, is_scratch_root};
use crate::diff::{
    self, BASELINE_SCHEMA, Baseline, COMPARED, CategorySet, Counts, Deviation, Entry, Gate, Keys, Samples, Side,
};
use crate::proc::{Captured, capture, find_on_path};
use crate::provenance;
use crate::run::table_cell;

/// skif rule to knip issue type.
const RULE_CATEGORIES: [(&str, &str); 14] = [
    ("unused-dependencies", "dependencies"),
    ("unused-dev-dependencies", "devDependencies"),
    ("unused-optional-peer-dependencies", "optionalPeerDependencies"),
    ("unlisted-dependencies", "unlisted"),
    ("unlisted-binaries", "binaries"),
    ("unresolved-imports", "unresolved"),
    ("unused-files", "files"),
    ("unused-exports", "exports"),
    ("unused-types", "types"),
    ("unused-namespace-exports", "nsExports"),
    ("unused-namespace-types", "nsTypes"),
    ("unused-enum-members", "enumMembers"),
    ("unused-namespace-members", "namespaceMembers"),
    ("duplicate-exports", "duplicates"),
];

/// knip issue type (as named in its JSON) to the common category. `nsExports` and `nsTypes` are their own categories
/// (the counterparts of `unused-namespace-exports` and `unused-namespace-types`, ADR 0029), not names of
/// `namespaceMembers`; knip's default run reports a namespace-only module's unused exports and types under `exports`
/// and `types` with a `namespace` field, which [`knip_keys`] moves into them. `classMembers` has no skif rule and is
/// left out of the comparison.
const KNIP_CATEGORIES: [(&str, &str); 14] = [
    ("dependencies", "dependencies"),
    ("devDependencies", "devDependencies"),
    ("optionalPeerDependencies", "optionalPeerDependencies"),
    ("unlisted", "unlisted"),
    ("binaries", "binaries"),
    ("unresolved", "unresolved"),
    ("files", "files"),
    ("exports", "exports"),
    ("types", "types"),
    ("enumMembers", "enumMembers"),
    ("namespaceMembers", "namespaceMembers"),
    ("nsExports", "nsExports"),
    ("nsTypes", "nsTypes"),
    ("duplicates", "duplicates"),
];

pub const NON_CODE_FILE: &str = "non-code-file";
pub const TEST_FILE: &str = "test-file";
pub const FIXTURE_FILE: &str = "fixture-file";
pub const KNIP_UNREAD: &str = "knip-unread";
pub const NOT_JUDGED: &str = "not-judged-by-skif";
pub const KNIP_CONFIG_ERROR: &str = "knip-config-error";
pub const FOREIGN_TSCONFIG_PATHS: &str = "foreign-tsconfig-paths";

/// The `detail` of an `unlisted-dependencies` finding whose import only another package's tsconfig `paths` map
/// (ADR 0075).
const FOREIGN_TSCONFIG_PATHS_DETAIL: &str = "ForeignTsconfigPaths";
pub const KNIP_UNJUDGED_MANIFEST: &str = "knip-unjudged-manifest";
pub const NON_MEMBER_PROJECT: &str = "non-member-project";
pub const SKIPPED_DIRECTORY: &str = "skipped-directory";
pub const PUBLISHED_SOURCE: &str = "published-source";
pub const IN_KNIP_UNUSED_FILE: &str = "in-knip-unused-file";
pub const IN_SKIF_UNUSED_FILE: &str = "in-skif-unused-file";
pub const USED_IN_KNIP_UNUSED_FILE: &str = "used-in-knip-unused-file";
pub const USED_IN_SKIF_UNUSED_FILE: &str = "used-in-skif-unused-file";
pub const RUSH_ROOT_FILE: &str = "rush-root-file";
pub const IMPORTED_BY_KNIP_UNUSED_FILE: &str = "imported-by-knip-unused-file";
pub const IMPORTED_BY_SKIF_UNUSED_FILE: &str = "imported-by-skif-unused-file";
pub const ENTRY_MODULE_DUPLICATE: &str = "entry-module-duplicate";
pub const BAZEL_BUILD_FILE: &str = "bazel-build-file";
pub const DECLARATION_FILE: &str = "declaration-file";

/// The dependency categories skif does not judge anywhere in a package whose entry files are not all known (ADR 0053),
/// whatever the package's `scope`.
const NOT_JUDGED_DEPENDENCY_CATEGORIES: [&str; 2] = ["dependencies", "devDependencies"];

/// The categories skif does not judge in the files of a package whose entry files are not all known: files (ADR 0053),
/// and exports, types and members, which skif stopped reporting there with #48 (ADR 0068). Where the package's `scope`
/// is narrowed to globs (`{"within": [...]}`), skif judges them in every file outside the globs (ADR 0065, ADR 0081).
const NOT_JUDGED_FILE_CATEGORIES: [&str; 7] = [
    "files",
    "exports",
    "types",
    "nsExports",
    "nsTypes",
    "enumMembers",
    "namespaceMembers",
];

/// The categories skif does not report in a fixture directory (ADR 0050): files, exports, types and members unless
/// `[js] include-fixtures` is set, which the harness never sets, and unresolved imports and unlisted dependencies at
/// all.
const FIXTURE_CATEGORIES: [&str; 9] = [
    "files",
    "exports",
    "types",
    "nsExports",
    "nsTypes",
    "enumMembers",
    "namespaceMembers",
    "unresolved",
    "unlisted",
];

/// The categories `non-member-project` leaves compared in a project that skif lints as a package of a root of a
/// recursive run (ADR 0022): skif judges the exports, types, members and duplicate exports of its files as knip does
/// from the enclosing workspace. Unused files, unresolved imports and unlisted dependencies there depend on the
/// project's own entry files, plugins and manifest, which knip does not read, so they stay set apart.
const ROOT_COMPARED_CATEGORIES: [&str; 7] = [
    "exports",
    "types",
    "nsExports",
    "nsTypes",
    "enumMembers",
    "namespaceMembers",
    "duplicates",
];

/// The categories whose findings name exports of a file, `<file>|<export>` or, for duplicates, `<file>|<names>`: what an
/// import of the file uses.
const IMPORTED_EXPORT_CATEGORIES: [&str; 5] = ["exports", "types", "nsExports", "nsTypes", "duplicates"];

/// The name that stands for every export of a file among the exports an import uses ([`unused_file_credits`]): a
/// namespace used as a whole, `import()` or `require` of the module, or a star re-export of it.
const WHOLE_MODULE: &str = "*";

/// The categories of a finding only skif reports that `non-member-project` sets apart below a project that is not one of
/// knip's workspaces: they follow from the project's own entry files, plugins, configs and manifest, which a recursive
/// skif run lints the project with and knip does not read there.
const NON_MEMBER_SKIF_CATEGORIES: [&str; 3] = ["files", "unlisted", "binaries"];

/// The files that declare a Bazel package (`BUILD_FILES` of `skif_js_plugins::bazel`).
const BUILD_FILES: [&str; 2] = ["BUILD.bazel", "BUILD"];

/// The files that mark the root of a Bazel repository (`ROOT_FILES` of `skif_js_plugins::bazel`).
const BAZEL_ROOT_FILES: [&str; 4] = ["MODULE.bazel", "REPO.bazel", "WORKSPACE.bazel", "WORKSPACE"];

/// What a root file of a Bazel repository that uses `rules_js` holds; skif's Bazel plugin reads no other repository.
const RULES_JS: &str = "aspect_rules_js";

/// The manifest categories of a listed package that an import makes used.
const IMPORTED_CATEGORIES: [&str; 3] = ["dependencies", "devDependencies", "optionalPeerDependencies"];

/// The categories whose findings sit on a `package.json`: its dependencies, and the binaries its scripts run.
const MANIFEST_CATEGORIES: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "optionalPeerDependencies",
    "binaries",
];

/// The dependency fields of a manifest that list a package.
const DEPENDENCY_FIELDS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// Categories that name the same thing, `<file>|<export>`, and differ only in how the tools label it: knip counts a
/// `const enum` as a type and files an export under `nsExports` when any importer uses a namespace, skif when all do.
const EXPORT_LABELS: [&str; 4] = ["exports", "types", "nsExports", "nsTypes"];

/// The known differences between skif and knip.
pub const KNIP_DEVIATIONS: [Deviation; 21] = [
    Deviation {
        id: NON_CODE_FILE,
        reason: "knip lists stylesheets, markup, data and docs as unused files; skif's project set is code files only",
    },
    Deviation {
        id: DECLARATION_FILE,
        reason: "knip reports these declaration files (`.d.ts`) as unused; skif never reports a `.d.ts` file as unused, since one holds ambient declarations or the typings a package publishes, which no import needs to name. skif's exemption covers `.d.ts` only, so a `.d.mts` or `.d.cts` file stays compared",
    },
    Deviation {
        id: TEST_FILE,
        reason: "knip reports these test files and files below test directories as unused and skif does not: its test-runner plugins make test files entries. The other files below test directories are test helpers, setup files and, in next-js (thousands), the app files of test fixtures, which are not test entries; the harness sets them apart by path without establishing why skif does not report each one",
    },
    Deviation {
        id: FIXTURE_FILE,
        reason: "knip reports these findings in files below a `fixtures` or `__fixtures__` directory, where skif reports no unused file, export, type or member unless `[js] include-fixtures` is set, and no unresolved import or unlisted dependency at all (ADR 0050). Other names of test data (`fixture`, `testdata`, `*-fixtures`) are judged like any other directory by skif and are not counted here",
    },
    Deviation {
        id: KNIP_UNREAD,
        reason: "knip reads no files below these paths (a deny-all .gitignore hides them), so it reports every dependency there as unused and has no view of their files",
    },
    Deviation {
        id: NOT_JUDGED,
        reason: "skif lists the package under `incomplete-packages`: a tool config it cannot read leaves its entry files unknown, so it does not judge the package's dependencies, nor its files, exports, types and members where the config could make entries: the whole package for a `scope` of `\"package\"`, only the files its globs match (relative to the package) for a `scope` of `{\"within\": [...]}`, among them the files of the packages nested below it, which skif makes entries (ADR 0065, ADR 0081)",
    },
    Deviation {
        id: KNIP_CONFIG_ERROR,
        reason: "knip could not load a configuration file of the package (it prints `ERROR: Error loading <file>` and exits with 2), so it ran without the entry files that configuration names: one-sided findings of either tool in that package are set apart. The errors are listed in `parity.json` under `knip_errors`",
    },
    Deviation {
        id: FOREIGN_TSCONFIG_PATHS,
        reason: "skif reports an import that only the tsconfig `paths` of another package map as `unlisted-dependencies` with the detail `ForeignTsconfigPaths`; knip merges the `paths` of all workspaces and resolves it (ADR 0075)",
    },
    Deviation {
        id: KNIP_UNJUDGED_MANIFEST,
        reason: "skif reports these unused dependencies, and binaries the scripts run, in a `package.json` that is not one of knip's workspaces: not the root manifest, not selected by the root's workspace patterns (`pnpm-workspace.yaml` or the `workspaces` of `package.json`, as skif's workspace discovery reads them) and not a file knip reports anything for. knip judges the dependencies and scripts of its workspaces only; a recursive skif run lints such a project as a root of its own (ADR 0022)",
    },
    Deviation {
        id: NON_MEMBER_PROJECT,
        reason: "knip reports these findings in a file (unused files, exports, types, members, duplicates and unresolved imports, and unlisted dependencies that the file's own `package.json` lists) below a directory with its own `package.json` (one with fields besides `type`, `sideEffects` and `imports`) that is not one of knip's workspaces (as in `knip-unjudged-manifest`). knip counts such files to an enclosing workspace, without the entry files, plugins and dependencies of their own manifest; skif's analysis of the enclosing workspace skips the directory, and a recursive run lints it as a root of its own when no workspace member encloses it (ADR 0022), where its exports, types, members and duplicates stay compared, but for the exports and types of the project's default entry files (`index`, `cli` or `main` at the top or in `src`), which are public in skif's run of the project and which knip reads without the project's entries. On skif's side, an unused file, unlisted dependency or unlisted binary only skif reports below such a project is set apart too: a recursive run lints the project with its own entry files, plugins, configs and manifest (a package its `.eslintrc.json` or `project.json` names), which knip does not read there; a binary on a `package.json` that knip reports anything on stays compared. The harness checks the manifests, not why skif has no finding for each one",
    },
    Deviation {
        id: SKIPPED_DIRECTORY,
        reason: "knip reports these findings in files below a directory that skif's file discovery skips by name, unless the file lies in a workspace member at or below that directory: `node_modules`, `.git`, `dist`, `.next`, `.turbo`, `.svelte-kit`, `storybook-static`, `.yarn`, `.vercel` and `.nuxt`; `out`, `target` and `coverage` directly in the root or a package directory; and every hidden directory but `.github`, `.storybook`, `.rnstorybook`, `.config`, `.vitepress`, `.vuepress`, `.husky` and `.changeset`. skif reads no file there",
    },
    Deviation {
        id: PUBLISHED_SOURCE,
        reason: "knip reports these files as unused, and each is either a TypeScript file whose package publishes its path with a built extension (`.js`, `.mjs`, `.cjs`, `.jsx` or a `.d.ts` variant) or without one in `main`, `module`, `source`, `types`, `typings`, `browser`, `bin`, an `exports` leaf or, in an n8n node package, `n8n.nodes` and `n8n.credentials`, where `*` matches any part of the path and a path below a build directory (`dist`, `lib`, `out`, `esm` and the like, also `lib-commonjs`) is also the path below `src` (for the n8n fields, the path without the build directory), or a file such a source imports by relative path, directly or through other files only knip reports unused. A published path without `*` is first mapped through the build directories of the package, as skif's TypeScript plugin reads them: below the `outDir` of a `tsconfig.json`, `tsconfig.*.json` or `jsconfig.json` of the package directory is the same path below its `rootDir` (without one, the directory its `include` and `files` share). The exports and types (also as namespace exports and types) only knip reports in a published source are `published-source` too: skif makes the source an entry, whose exports are public. skif makes the source of a published path an entry and uses what it imports; knip maps a published path to source only through a tsconfig `outDir` it resolves itself and does not read the n8n fields. The harness checks the published path and the relative imports, not why skif has no finding for each file; enum and namespace members of these files, and the exports of the files they reach, stay compared, and their duplicates are `entry-module-duplicate`",
    },
    Deviation {
        id: ENTRY_MODULE_DUPLICATE,
        reason: "knip reports these duplicate exports (one value exported under several names, `export const x …; export default x`) in a file skif makes an entry from the manifest and the defaults alone: a default entry file of its project (`index`, `cli` or `main` at the top or in `src`) or the source of a path its manifest publishes (as in `published-source`). skif's `duplicate-exports` skips entry modules, whose names are all public API; knip reports them. Entries that only skif's plugins or the configuration make are not known to the harness, so duplicates there stay compared",
    },
    Deviation {
        id: BAZEL_BUILD_FILE,
        reason: "skif's Bazel plugin reads the `BUILD.bazel` and `BUILD` files of a repository that uses `rules_js`, which knip has no plugin for: skif reports these unlisted dependencies at a `BUILD` file that names a package by label, and credits these dependencies knip reports as unused through a label of a `BUILD` file in the package's directory: a `node_modules` target of the package's Bazel package (`\":node_modules/zod\"`, `\"//:node_modules/typescript\"`) or the `package_json.bzl` of another repository (`\"@npm//packages/app:esbuild/package_json.bzl\"`), the harness scanning the string literals of every `BUILD` file below a Bazel root that names `aspect_rules_js` as skif does. The files skif makes entries from the `srcs`, `data` and `entry_point` of the rules, and their exports, stay compared",
    },
    Deviation {
        id: IN_KNIP_UNUSED_FILE,
        reason: "skif reports these findings (anything but unused files and manifest findings) in a file that knip lists as unused. knip does not analyse an unused file, so it reports nothing inside one; the disagreement is the file, which `files` already counts once",
    },
    Deviation {
        id: IN_SKIF_UNUSED_FILE,
        reason: "knip reports these findings (anything but unused files and manifest findings) in a file that skif reports as unused, so skif reports nothing inside it; the disagreement is the file, which `files` already counts once",
    },
    Deviation {
        id: RUSH_ROOT_FILE,
        reason: "knip reports these findings in files of a Rush repository's root that lie in no project (`.prettierrc.js`, `common/scripts`, `common/config`): the root `package.json` the harness writes for knip (`knip-root = \"rush\"`) makes the root a workspace, while a Rush root has no `package.json`, so skif analyses no file there",
    },
    Deviation {
        id: USED_IN_KNIP_UNUSED_FILE,
        reason: "knip reports these dependencies as unused, and a file of the same workspace that knip lists as unused and skif does not imports the package (an import scan of those files with skif's parser; an `@types/` package counts as imported with its runtime package). knip does not analyse an unused file, so it never sees the import skif credits; the disagreement is the file, which `files` already counts once. The harness does not check that the scanned import is the one skif credits",
    },
    Deviation {
        id: USED_IN_SKIF_UNUSED_FILE,
        reason: "skif reports these dependencies as unused, and a file of the same workspace that skif reports as unused and knip does not imports the package (the same import scan; an `@types/` package counts as imported with its runtime package). skif credits no import of a file it calls unused, while knip analyses the file and credits the import; the disagreement is the file, which `files` already counts once. The harness does not check that the scanned import is the one knip credits",
    },
    Deviation {
        id: IMPORTED_BY_KNIP_UNUSED_FILE,
        reason: "knip reports these exports, types and duplicate exports as unused, and a file that knip lists as unused and skif does not imports them by relative path: by name, through a namespace whose members it reads, or as a whole module (a namespace used as a whole, `import()`, `require`), directly or through the `export … from` and `export *` re-exports of the files in between (an import scan of those files with skif's parser). knip does not analyse an unused file, so it never sees the import skif credits; the disagreement is the importing file, which `files` already counts once. The in-file counterpart of `used-in-knip-unused-file`; the harness does not check that skif credits the export through that import",
    },
    Deviation {
        id: IMPORTED_BY_SKIF_UNUSED_FILE,
        reason: "skif reports these exports, types and duplicate exports as unused, and a file that skif reports as unused and knip does not imports them (the same import scan). skif credits no import of a file it calls unused, while knip analyses the file and credits the import; the disagreement is the importing file, which `files` already counts once. The in-file counterpart of `used-in-skif-unused-file`",
    },
];

/// Extensions skif parses and follows; every other file is outside its project set.
const CODE_EXTENSIONS: [&str; 11] = [
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "vue", "svelte", "astro",
];

const TEST_DIRECTORIES: [&str; 6] = ["test", "tests", "__tests__", "e2e", "cypress", "__mocks__"];
/// The directories skif's JS analysis treats as fixtures (`skif_config::FIXTURE_DIRECTORIES`, ADR 0050).
const FIXTURE_DIRECTORIES: [&str; 2] = ["fixtures", "__fixtures__"];

/// The default entry files of a workspace, the same in knip and skif. A configured `entry` replaces them in both, so a
/// corpus-level `entry` is given with these in front.
const DEFAULT_ENTRY: [&str; 2] = [
    "{index,cli,main}.{js,mjs,cjs,jsx,ts,tsx,mts,cts}",
    "src/{index,cli,main}.{js,mjs,cjs,jsx,ts,tsx,mts,cts}",
];

/// Marker written into a checkout once its dependencies are installed, holding the pinned commit.
const INSTALLED_MARKER: &str = ".skif-bench-installed";

// Independent command-line switches, one per flag.
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    pub skif: PathBuf,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub out: PathBuf,
    pub only: Vec<String>,
    pub install: bool,
    pub recursive: bool,
    /// Run knip with the repository's own configuration instead of an empty one.
    pub repo_knip_config: bool,
    /// Count test and fixture files in the knip-only `files` instead of reporting them as known deviations.
    pub include_test_files: bool,
    pub timeout: Duration,
    pub gate: Gate,
}

/// Makes a path relative to the checkout with `/` separators.
pub fn normalize(raw: &str, root: &Path) -> String {
    let unified = raw.replace('\\', "/");
    let prefix = format!("{}/", root.to_string_lossy().replace('\\', "/").trim_end_matches('/'));
    let relative = unified.strip_prefix(&prefix).unwrap_or(&unified);
    relative.trim_start_matches("./").to_owned()
}

/// The backtick-quoted names in a diagnostic message.
fn backticked(message: &str) -> Vec<&str> {
    message.split('`').skip(1).step_by(2).collect()
}

/// The name of an enum or namespace member without its `Enum.` or `ns.` qualifier: skif names `Enum.Member`, knip
/// names `Member`.
fn member_name(category: &str, name: &str) -> String {
    if matches!(category, "enumMembers" | "namespaceMembers") {
        name.rsplit('.').next().unwrap_or(name).to_owned()
    } else {
        name.to_owned()
    }
}

fn joined_sorted(mut names: Vec<String>) -> String {
    names.sort();
    names.join("|")
}

/// The findings of a skif JSON document, by knip issue type.
pub fn skif_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    let Some(diagnostics) = document.get("diagnostics").and_then(Value::as_array) else {
        return keys;
    };
    for diagnostic in diagnostics {
        let Some(rule) = diagnostic.get("rule").and_then(Value::as_str) else {
            continue;
        };
        let Some((_, category)) = RULE_CATEGORIES.iter().find(|(id, _)| *id == rule) else {
            continue;
        };
        let key = skif_key(diagnostic, category, root);
        keys.entry((*category).to_owned()).or_default().insert(key);
    }
    keys
}

/// The key of a skif diagnostic in `category`: `<file>` for files, `<file>|<names>` for duplicates and `<file>|<name>`
/// otherwise.
fn skif_key(diagnostic: &Value, category: &str, root: &Path) -> String {
    let path = normalize(
        diagnostic
            .pointer("/location/path")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        root,
    );
    let message = diagnostic.get("message").and_then(Value::as_str).unwrap_or_default();
    let names = backticked(message);
    match category {
        "files" => path,
        "duplicates" => format!(
            "{path}|{}",
            joined_sorted(names.iter().skip(1).map(|n| (*n).to_owned()).collect())
        ),
        _ => {
            let name = diagnostic
                .get("dependency")
                .and_then(Value::as_str)
                .or_else(|| names.first().copied())
                .unwrap_or_default();
            format!("{path}|{}", member_name(category, name))
        }
    }
}

/// The keys of the `unlisted-dependencies` findings with the detail `ForeignTsconfigPaths`: imports that only the
/// tsconfig `paths` of another package map, which knip resolves through the merged `paths` of all workspaces.
pub fn foreign_tsconfig_paths_keys(document: &Value, root: &Path) -> BTreeSet<String> {
    document
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|diagnostic| {
            diagnostic.get("rule").and_then(Value::as_str) == Some("unlisted-dependencies")
                && diagnostic.get("detail").and_then(Value::as_str) == Some(FOREIGN_TSCONFIG_PATHS_DETAIL)
        })
        .map(|diagnostic| skif_key(diagnostic, "unlisted", root))
        .collect()
}

fn name_of(value: &Value) -> Option<String> {
    value
        .get("name")
        .or_else(|| value.get("symbol"))
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(str::to_owned)
}

/// The names of one item of a knip issue type: a group of items (duplicates) is one joined name.
fn item_names(item: &Value) -> Vec<String> {
    match item {
        Value::Array(group) => vec![joined_sorted(group.iter().filter_map(name_of).collect())],
        other => name_of(other).into_iter().collect(),
    }
}

/// The names under one knip issue type: a list of items, a list of lists (duplicates) or a map of lists (enums).
fn knip_names(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().flat_map(item_names).collect(),
        Value::Object(map) => map.values().flat_map(knip_names).collect(),
        _ => Vec::new(),
    }
}

/// The category a knip item of `exports` or `types` that carries a `namespace` belongs to: knip names the namespace
/// import that reads the module (`parentSymbol`) on the findings of a module that is only read through one.
fn namespaced_category(type_name: &str, item: &Value) -> Option<&'static str> {
    let namespaced = item.get("namespace").is_some_and(Value::is_string);
    match type_name {
        "exports" if namespaced => Some("nsExports"),
        "types" if namespaced => Some("nsTypes"),
        _ => None,
    }
}

/// The findings of a knip JSON report, by issue type.
pub fn knip_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    if let Some(files) = document.get("files").and_then(Value::as_array) {
        keys.entry("files".to_owned())
            .or_default()
            .extend(files.iter().filter_map(Value::as_str).map(|f| normalize(f, root)));
    }
    let Some(issues) = document.get("issues").and_then(Value::as_array) else {
        return keys;
    };
    for issue in issues {
        let file = normalize(issue.get("file").and_then(Value::as_str).unwrap_or_default(), root);
        for (type_name, category) in KNIP_CATEGORIES {
            let Some(value) = issue.get(type_name) else {
                continue;
            };
            let routed: Vec<(&str, String)> = match value {
                Value::Array(items) => items
                    .iter()
                    .flat_map(|item| {
                        let target = namespaced_category(type_name, item).unwrap_or(category);
                        item_names(item).into_iter().map(move |name| (target, name))
                    })
                    .collect(),
                other => knip_names(other).into_iter().map(|name| (category, name)).collect(),
            };
            for (target, name) in routed {
                let set = keys.entry(target.to_owned()).or_default();
                if target == "files" {
                    set.insert(file.clone());
                } else {
                    set.insert(format!("{file}|{}", member_name(target, &name)));
                }
            }
        }
    }
    keys
}

/// Moves each export finding of `skif` that knip has under another label to that label, so the same `<file>|<export>`
/// counts as agreement.
fn align_export_labels(skif: &mut Keys, knip: &Keys) {
    for category in EXPORT_LABELS {
        let Some(keys) = skif.get(category) else { continue };
        let moved: Vec<(String, &str)> = keys
            .iter()
            .filter(|key| !knip.get(category).is_some_and(|own| own.contains(*key)))
            .filter_map(|key| {
                EXPORT_LABELS
                    .iter()
                    .find(|other| knip.get(**other).is_some_and(|theirs| theirs.contains(key)))
                    .map(|other| (key.clone(), *other))
            })
            .collect();
        for (key, label) in moved {
            if let Some(keys) = skif.get_mut(category) {
                keys.remove(&key);
            }
            skif.entry(label.to_owned()).or_default().insert(key);
        }
    }
}

/// Moves each finding only skif reports in `unlisted` or `unresolved` that knip reports in the other of the two, on the
/// same file and package, to knip's category and key, so it counts as agreement: both tools report the import (a
/// tsconfig `types` entry, `importHelpers`), skif as a package no manifest lists and knip as a module it cannot
/// resolve, or the reverse. A specifier with a subpath (`vite/client`) pairs with its package (`vite`); each knip
/// finding pairs with one skif finding at most.
fn align_unlisted_unresolved(skif: &mut Keys, knip: &Keys) {
    let empty = BTreeSet::new();
    let mut paired: BTreeSet<(&str, String)> = BTreeSet::new();
    let mut moves: Vec<(&str, String, &str, String)> = Vec::new();
    for (own, other) in [("unlisted", "unresolved"), ("unresolved", "unlisted")] {
        let knip_own = knip.get(own).unwrap_or(&empty);
        let knip_other = knip.get(other).unwrap_or(&empty);
        let skif_other = skif.get(other).unwrap_or(&empty);
        for key in skif.get(own).unwrap_or(&empty).difference(knip_own) {
            let Some((path, name)) = key.split_once('|') else {
                continue;
            };
            let Some(package) = bare_package(name) else {
                continue;
            };
            let found = knip_other.iter().find(|candidate| {
                !skif_other.contains(*candidate)
                    && !paired.contains(&(other, (*candidate).clone()))
                    && candidate
                        .split_once('|')
                        .is_some_and(|(file, theirs)| file == path && bare_package(theirs) == Some(package))
            });
            if let Some(found) = found {
                paired.insert((other, found.clone()));
                moves.push((own, key.clone(), other, found.clone()));
            }
        }
    }
    for (own, key, other, found) in moves {
        if let Some(keys) = skif.get_mut(own) {
            keys.remove(&key);
        }
        skif.entry(other.to_owned()).or_default().insert(found);
    }
}

/// What skif left unjudged in a package it lists under `incomplete-packages`, from the entry's `scope`.
#[derive(Debug, Clone)]
pub enum NotJudgedScope {
    /// `"package"` (or no `scope`): no file of the package is judged.
    Package,
    /// `{"within": [globs]}`: only the files these globs match, relative to the package directory, are not judged.
    /// The globs are compiled once, as skif's analysis compiles them.
    Within(IncompleteScopeGlobs),
}

impl PartialEq for NotJudgedScope {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Package, Self::Package) => true,
            (Self::Within(ours), Self::Within(theirs)) => ours.patterns() == theirs.patterns(),
            _ => false,
        }
    }
}

impl Eq for NotJudgedScope {}

impl NotJudgedScope {
    /// A scope narrowed to `globs`.
    fn within(globs: Vec<String>) -> Self {
        Self::Within(IncompleteScopeGlobs::new(globs))
    }

    fn read(value: Option<&Value>) -> Self {
        let globs = value.and_then(|scope| scope.get("within")).and_then(Value::as_array);
        globs.map_or(Self::Package, |globs| {
            Self::within(globs.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        })
    }

    /// The scope covering both: the whole package when either is (as `skif_core::IncompleteScope::union`).
    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Within(ours), Self::Within(more)) => {
                let mut globs = ours.patterns().to_vec();
                for glob in more.patterns() {
                    if !globs.contains(glob) {
                        globs.push(glob.clone());
                    }
                }
                Self::within(globs)
            }
            _ => Self::Package,
        }
    }
}

/// The packages skif did not judge (its JSON `incomplete-packages`) with what it left unjudged in each: by directory,
/// relative to the checkout with `/` separators, the root package as the empty string. A directory listed twice (by
/// two roots of a recursive run) gets the union of its scopes.
pub fn not_judged_dirs(document: &Value) -> BTreeMap<String, NotJudgedScope> {
    let mut packages: BTreeMap<String, NotJudgedScope> = BTreeMap::new();
    for package in document
        .get("incomplete-packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(dir) = package.get("dir").and_then(Value::as_str) else {
            continue;
        };
        let dir = if dir == "." {
            String::new()
        } else {
            dir.trim_end_matches('/').to_owned()
        };
        let scope = NotJudgedScope::read(package.get("scope"));
        let merged = match packages.remove(&dir) {
            Some(known) => known.union(scope),
            None => scope,
        };
        packages.insert(dir, merged);
    }
    packages
}

/// The package directories of every root of a recursive skif run (the `packages` of its JSON `roots`, ADR 0022),
/// relative to the checkout with `/` separators, the root package as the empty string. Empty for a single-workspace
/// run, whose report has no `roots`.
pub fn skif_root_packages(document: &Value) -> BTreeSet<String> {
    document
        .get("roots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|root| root.get("packages").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(|dir| match dir.replace('\\', "/").trim_end_matches('/') {
            "." => String::new(),
            dir => dir.trim_start_matches("./").to_owned(),
        })
        .collect()
}

/// The package that owns a finding key (`<path>` or `<path>|<name>`): the deepest directory above the path that
/// `is_package` accepts (for the classification, [`Layout::is_project`], so a marker-only manifest owns nothing), the
/// root (`""`) at the latest.
fn owner_dir(key: &str, is_package: &dyn Fn(&str) -> bool) -> String {
    let path = key.split_once('|').map_or(key, |(path, _)| path);
    let mut dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    while !dir.is_empty() && !is_package(dir) {
        dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    dir.to_owned()
}

/// Whether a one-sided finding is in a category skif does not judge where it lies: a dependency of a package skif
/// lists as incomplete, or a file, export, type or member in its files, all of them or only those its narrowed scope
/// matches. A narrowed scope also reaches the files of the packages nested below the incomplete package, matched
/// relative to the incomplete package, as skif makes every file it matches an entry (ADR 0081).
fn not_judged(
    category: &str,
    key: &str,
    packages: &BTreeMap<String, NotJudgedScope>,
    is_package: &dyn Fn(&str) -> bool,
) -> bool {
    let in_files = NOT_JUDGED_FILE_CATEGORIES.contains(&category);
    if packages.is_empty() || !(in_files || NOT_JUDGED_DEPENDENCY_CATEGORIES.contains(&category)) {
        return false;
    }
    let dir = owner_dir(key, is_package);
    let path = key_path(key);
    let own = packages.get(&dir).is_some_and(|scope| {
        !in_files
            || match scope {
                NotJudgedScope::Package => true,
                NotJudgedScope::Within(globs) => globs.is_match(below(&dir, path)),
            }
    });
    own || (in_files && in_enclosing_scope(&dir, path, packages))
}

/// `path` relative to the directory `dir` at or above it (`""` is the root, and `dir` itself).
fn below<'p>(dir: &str, path: &'p str) -> &'p str {
    if dir.is_empty() {
        return path;
    }
    path.strip_prefix(dir)
        .and_then(|rest| if rest.is_empty() { Some(rest) } else { rest.strip_prefix('/') })
        .unwrap_or(path)
}

/// `name` joined to the directory `dir`, both relative to the checkout; either may be empty.
fn joined(dir: &str, name: &str) -> String {
    match (dir, name) {
        ("", name) => name.to_owned(),
        (dir, "") => dir.to_owned(),
        (dir, name) => format!("{dir}/{name}"),
    }
}

/// Whether the narrowed scope of an incomplete package above the package directory `dir` matches `path`, relative to
/// that package (`scope_entries` in `skif-js`).
fn in_enclosing_scope(dir: &str, path: &str, packages: &BTreeMap<String, NotJudgedScope>) -> bool {
    let mut above = dir;
    while !above.is_empty() {
        above = parent_dir(above);
        if let Some(NotJudgedScope::Within(globs)) = packages.get(above)
            && globs.is_match(below(above, path))
        {
            return true;
        }
    }
    false
}

/// The known deviation a finding only skif reports belongs to: an unlisted import that only another package's tsconfig
/// `paths` map (`foreign`, from [`foreign_tsconfig_paths_keys`]), which knip resolves through the merged `paths`.
fn skif_only_deviation(category: &str, key: &str, foreign: &BTreeSet<String>) -> Option<&'static str> {
    (category == "unlisted" && foreign.contains(key)).then_some(FOREIGN_TSCONFIG_PATHS)
}

/// Whether a knip-reported unused file is a known deviation: outside skif's project set (not code), a declaration file
/// skif never reports (`.d.ts`, as `unused-files` filters them), in a directory skif treats as fixtures, a test file or
/// any code file below a test directory (such as the apps of an end-to-end suite). Fixtures and tests only count when
/// `include_test_files` is off.
fn classify_file(path: &str, include_test_files: bool) -> Option<&'static str> {
    let normalized = path.replace('\\', "/");
    let (directories, name) = normalized.rsplit_once('/').unwrap_or(("", normalized.as_str()));
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if !extension.is_some_and(|e| CODE_EXTENSIONS.contains(&e.as_str())) {
        return Some(NON_CODE_FILE);
    }
    if name.ends_with(".d.ts") {
        return Some(DECLARATION_FILE);
    }
    if include_test_files {
        return None;
    }
    if in_fixture_directory(&normalized) {
        return Some(FIXTURE_FILE);
    }
    let in_test_directory = directories.split('/').any(|p| TEST_DIRECTORIES.contains(&p));
    let test_name = [".test.", ".spec.", "_spec.", "_test."]
        .iter()
        .any(|marker| name.contains(marker));
    (in_test_directory || test_name).then_some(TEST_FILE)
}

/// The file of a finding key (`<path>` or `<path>|<name>`).
fn key_path(key: &str) -> &str {
    key.split_once('|').map_or(key, |(path, _)| path)
}

/// Whether a directory of `path` (not its file name) is one skif treats as fixtures.
fn in_fixture_directory(path: &str) -> bool {
    path.rsplit_once('/')
        .is_some_and(|(directories, _)| directories.split('/').any(|part| FIXTURE_DIRECTORIES.contains(&part)))
}

/// The package a bare specifier names: `@scope/name` or `name`, without a subpath.
fn package_name(specifier: &str) -> &str {
    let segments = if specifier.starts_with('@') { 2 } else { 1 };
    specifier
        .match_indices('/')
        .nth(segments - 1)
        .map_or(specifier, |(at, _)| &specifier[..at])
}

/// Directory names skif's file discovery never enters unless the directory is a workspace member (`SKIPPED_DIRS` in
/// `crates/skif-js/src/discover.rs`).
const SKIPPED_DIRECTORIES: [&str; 13] = [
    "node_modules",
    ".git",
    "dist",
    "coverage",
    ".next",
    "out",
    ".turbo",
    ".svelte-kit",
    "storybook-static",
    ".yarn",
    ".vercel",
    ".nuxt",
    "target",
];

/// The names of [`SKIPPED_DIRECTORIES`] that are skipped only directly inside a package directory
/// (`PACKAGE_ROOT_ONLY_DIRS` in `discover.rs`).
const PACKAGE_ROOT_ONLY_DIRECTORIES: [&str; 3] = ["out", "target", "coverage"];

/// The hidden directories skif's file discovery enters (`HIDDEN_DIRS` in `discover.rs`); it skips every other one.
const TOOL_DIRECTORIES: [&str; 8] = [
    ".storybook",
    ".rnstorybook",
    ".config",
    ".vitepress",
    ".vuepress",
    ".husky",
    ".github",
    ".changeset",
];

/// Manifest fields that only mark a directory: a `package.json` with nothing else is not a project
/// (`MARKER_FIELDS` in `discover.rs`).
const MARKER_FIELDS: [&str; 3] = ["type", "sideEffects", "imports"];

/// Manifest fields that publish one file each.
const PUBLISHED_FIELDS: [&str; 6] = ["main", "module", "source", "types", "typings", "browser"];

/// The extensions of a published (built) path and the TypeScript source extensions skif maps each one back to
/// (`EXTENSION_PAIRS` in `crates/skif-js/src/manifest_paths.rs`, the TypeScript sources only).
const PUBLISHED_EXTENSIONS: [(&str, &[&str]); 7] = [
    (".d.mts", &["mts"]),
    (".d.cts", &["cts"]),
    (".d.ts", &["ts", "tsx"]),
    (".mjs", &["mts", "ts", "tsx", "cts"]),
    (".cjs", &["cts", "ts", "tsx", "mts"]),
    (".jsx", &["tsx"]),
    (".js", &["ts", "tsx", "mts", "cts"]),
];

/// The TypeScript source extensions; an extensionless published path (`./out/main`) may be the twin of any of them.
const TYPESCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts"];

/// The stem and extension of a TypeScript source file (`.ts`, `.tsx`, `.mts`, `.cts`, not a declaration file).
fn typescript_source(path: &str) -> Option<(&str, &str)> {
    if is_declaration(path) {
        return None;
    }
    let (stem, extension) = path.rsplit_once('.')?;
    TYPESCRIPT_EXTENSIONS.contains(&extension).then_some((stem, extension))
}

/// Whether `text` matches `pattern`, where each `*` matches one or more characters, `/` included, as in the
/// subpath patterns of `exports`.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => text.strip_prefix(head).is_some_and(|rest| {
            rest.char_indices()
                .map(|(at, character)| at + character.len_utf8())
                .any(|end| wildcard_match(tail, &rest[end..]))
        }),
    }
}

/// The published path without its built extension and the source extensions it maps back to: those of
/// [`PUBLISHED_EXTENSIONS`], or every TypeScript one for a path whose file name has no extension (`./out/main`, which
/// Node resolves with `.js`). `None` for any other extension (`.css`, `.json`).
fn published_stem(clean: &str) -> Option<(&str, &[&str])> {
    PUBLISHED_EXTENSIONS
        .iter()
        .find_map(|(suffix, sources)| clean.strip_suffix(suffix).map(|published| (published, *sources)))
        .or_else(|| {
            let name = clean.rsplit('/').next().unwrap_or(clean);
            (!name.contains('.')).then_some((clean, TYPESCRIPT_EXTENSIONS))
        })
}

/// Whether the source file `stem.extension` (relative to its package) is a twin of the published path pattern
/// `target` (one with `*`): the same path, or for a path below a build directory (`is_out_dir` of `skif-js`: `out`,
/// `lib-commonjs`, ...) the same path below `src`. skif makes every file such a pattern matches an entry; a published
/// path without `*` names one source only ([`Layout::source_of`]).
fn is_published_twin(target: &str, stem: &str, extension: &str) -> bool {
    let clean = target.trim_start_matches("./");
    let Some((published, sources)) = published_stem(clean) else {
        return false;
    };
    if !sources.contains(&extension) {
        return false;
    }
    let below_src = published
        .split_once('/')
        .filter(|(first, _)| is_out_dir(first))
        .map(|(_, rest)| format!("src/{rest}"));
    wildcard_match(published, stem) || below_src.is_some_and(|source| wildcard_match(&source, stem))
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.is_empty() => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// The file paths a manifest publishes: [`PUBLISHED_FIELDS`], every `bin`, every `exports` leaf but the catch-all `./*`
/// and the node and credential files of an n8n node package as skif reads them (`skif_js::n8n_load_paths`).
fn published_paths(manifest: &Value) -> Vec<String> {
    let mut paths: Vec<String> = PUBLISHED_FIELDS
        .iter()
        .filter_map(|field| manifest.get(*field).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    if let Some(bin) = manifest.get("bin") {
        collect_strings(bin, &mut paths);
    }
    if let Some(exports) = manifest.get("exports") {
        let mut leaves = Vec::new();
        collect_strings(exports, &mut leaves);
        paths.extend(
            leaves
                .into_iter()
                .filter(|leaf| leaf.starts_with("./") && leaf != "./*"),
        );
    }
    paths.extend(n8n_load_paths(manifest));
    paths
}

/// The extensions a relative specifier without one resolves with, in the order skif's resolver tries them.
const RESOLVED_EXTENSIONS: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

/// The JavaScript extensions a relative specifier may name for its TypeScript twin (`./a.js` for `a.ts`).
const BUILT_EXTENSIONS: [&str; 4] = [".js", ".jsx", ".mjs", ".cjs"];

/// `specifier` (`./a`, `../b/c.js`) joined to the directory `dir`, both relative to the checkout; `None` when it leaves
/// the checkout.
fn join_relative(dir: &str, specifier: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// The file a relative specifier in a file of `dir` names: the path itself, the path with one of
/// [`RESOLVED_EXTENSIONS`] (a [`BUILT_EXTENSIONS`] one also replaced by them), or the `index` file of the directory it
/// names.
fn resolve_relative(dir: &str, specifier: &str, is_file: &FileProbe<'_>) -> Option<String> {
    let path = join_relative(dir, specifier)?;
    let stem = BUILT_EXTENSIONS
        .iter()
        .find_map(|extension| path.strip_suffix(extension))
        .unwrap_or(&path);
    let index = if path.is_empty() {
        "index".to_owned()
    } else {
        format!("{path}/index")
    };
    std::iter::once(path.clone())
        .chain(RESOLVED_EXTENSIONS.iter().map(|extension| format!("{stem}{extension}")))
        .chain(
            RESOLVED_EXTENSIONS
                .iter()
                .map(|extension| format!("{index}{extension}")),
        )
        .find(|candidate| is_file(candidate))
}

/// Whether a module specifier is relative (`.`, `..`, `./a`, `../b`) and has no query (`?raw`), which names no module.
fn is_relative_specifier(specifier: &str) -> bool {
    !specifier.contains('?')
        && (matches!(specifier, "." | "..") || specifier.starts_with("./") || specifier.starts_with("../"))
}

/// The directory of a path relative to the checkout (`""` for a file of the root).
fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The imports of the parsed file `path` that name a file by relative specifier, with that file: its imports,
/// requires, re-exports and dynamic imports; globs, test-runner mocks and specifiers with a query are left out.
fn relative_targets<'m>(path: &str, module: &'m ModuleInfo, is_file: &FileProbe<'_>) -> Vec<(&'m Import, String)> {
    let dir = parent_dir(path);
    module
        .imports
        .iter()
        .filter(|import| {
            !matches!(
                import.kind,
                ImportKind::ImportMetaGlob | ImportKind::SourceGlob | ImportKind::Mock
            ) && is_relative_specifier(&import.specifier)
        })
        .filter_map(|import| resolve_relative(dir, &import.specifier, is_file).map(|target| (import, target)))
        .collect()
}

/// The files a source file imports by relative specifier, as skif's parser reads its imports, requires, re-exports and
/// dynamic imports ([`relative_targets`]). Empty for a file it cannot parse.
fn relative_imports(path: &str, source: &str, is_file: &FileProbe<'_>) -> Vec<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let Ok(module) = parse_module(Path::new(path), source, &options) else {
        return Vec::new();
    };
    relative_targets(path, &module, is_file)
        .into_iter()
        .map(|(_, target)| target)
        .collect()
}

/// The exports of other files that the parsed file `path` imports by relative specifier, as `(file, export)`: a named
/// import or re-export by its name, a default import as `default`, a namespace by the members read through it, and a
/// namespace used as a whole, `export * from`, `import()` and `require` of the module as [`WHOLE_MODULE`]. A
/// side-effect import uses no export.
fn imported_exports(path: &str, module: &ModuleInfo, is_file: &FileProbe<'_>) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (import, target) in relative_targets(path, module, is_file) {
        if import.names.is_empty() {
            if matches!(import.kind, ImportKind::Dynamic | ImportKind::Require) {
                found.push((target, WHOLE_MODULE.to_owned()));
            }
            continue;
        }
        for name in &import.names {
            match &name.kind {
                ImportedKind::Named(export) => found.push((target.clone(), export.clone())),
                ImportedKind::Default => found.push((target.clone(), "default".to_owned())),
                ImportedKind::Namespace if name.local.is_some() && !name.escapes => found.extend(
                    name.accessed_members
                        .iter()
                        .map(|member| (target.clone(), member.clone())),
                ),
                ImportedKind::Namespace => found.push((target.clone(), WHOLE_MODULE.to_owned())),
            }
        }
    }
    found
}

/// Where the export `name` (or, for [`WHOLE_MODULE`], every export) of the parsed file `path` comes from when the file
/// re-exports it, as `(file, export)`: the source of `export { name } from` (and of `import { x } from`; `export { x }`)
/// under the name taken from it, and the source of an `export * from` for a name the file does not export itself.
fn reexported(path: &str, module: &ModuleInfo, name: &str, is_file: &FileProbe<'_>) -> Vec<(String, String)> {
    let declares = |wanted: &str| module.exports.iter().any(|export| export.name == wanted);
    module
        .exports
        .iter()
        .filter_map(|export| {
            let specifier = export.from.as_deref().filter(|from| is_relative_specifier(from))?;
            let followed = if export.kind == ExportKind::StarReExport && export.name == WHOLE_MODULE {
                (name == WHOLE_MODULE || !declares(name)).then(|| name.to_owned())
            } else {
                (name == WHOLE_MODULE || export.name == name)
                    .then(|| export.imported_name.clone().unwrap_or_else(|| export.name.clone()))
            };
            followed.and_then(|followed| {
                resolve_relative(parent_dir(path), specifier, is_file).map(|target| (target, followed))
            })
        })
        .collect()
}

/// `<file>|<export>` (`<file>|*` for [`WHOLE_MODULE`]) for every export that one of the files only one tool lists as
/// unused (`unused` but not `other_unused`) imports by relative specifier ([`imported_exports`]), and for the exports
/// those re-export from other files, followed through every file in between ([`reexported`]): what that tool would
/// have credited had it analysed the file. `read` gives a file's source; a file it cannot read or parse imports
/// nothing and re-exports nothing.
fn unused_file_credits(
    unused: &BTreeSet<String>,
    other_unused: &BTreeSet<String>,
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let parse = |file: &str| read(file).and_then(|source| parse_module(Path::new(file), &source, &options).ok());
    let mut pending: Vec<(String, String)> = unused
        .difference(other_unused)
        .filter_map(|file| parse(file).map(|module| imported_exports(file, &module, is_file)))
        .flatten()
        .collect();
    let mut modules: BTreeMap<String, Option<ModuleInfo>> = BTreeMap::new();
    let mut credits = BTreeSet::new();
    while let Some((file, name)) = pending.pop() {
        if !credits.insert(format!("{file}|{name}")) {
            continue;
        }
        if let Some(module) = modules.entry(file.clone()).or_insert_with(|| parse(&file)) {
            pending.extend(reexported(&file, module, &name, is_file));
        }
    }
    credits
}

/// Whether a finding names an export of its file (or, for duplicates, one of its names) that `credits` (from
/// [`unused_file_credits`]) holds, or the file is credited as a whole.
fn imported_by_unused_file(credits: &BTreeSet<String>, category: &str, key: &str) -> bool {
    if !IMPORTED_EXPORT_CATEGORIES.contains(&category) {
        return false;
    }
    let Some((path, names)) = key.split_once('|') else {
        return false;
    };
    credits.contains(&format!("{path}|{WHOLE_MODULE}"))
        || names
            .split('|')
            .any(|name| credits.contains(&format!("{path}|{name}")))
}

/// The files only knip reports as unused (`unused` but not `other_unused`) that one of them whose path its package
/// publishes ([`Layout::publishes_source_of`]) reaches by relative imports ([`relative_imports`]), directly or through
/// other such files; the published sources themselves are left out. skif makes a published source an entry, so it
/// uses every file the entry imports; knip, which reads no entry there, reports the whole chain. `read` gives a file's
/// source; a file it cannot read imports nothing.
fn published_reach(
    unused: &BTreeSet<String>,
    other_unused: &BTreeSet<String>,
    layout: &Layout<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let only: BTreeSet<&str> = unused.difference(other_unused).map(String::as_str).collect();
    let mut pending: Vec<&str> = only
        .iter()
        .copied()
        .filter(|file| layout.publishes_source_of(file))
        .collect();
    let mut seen: BTreeSet<&str> = pending.iter().copied().collect();
    let mut reached = BTreeSet::new();
    while let Some(file) = pending.pop() {
        let Some(source) = read(file) else { continue };
        for target in relative_imports(file, &source, layout.is_file) {
            if let Some(&found) = only.get(target.as_str())
                && seen.insert(found)
            {
                reached.insert(found.to_owned());
                pending.push(found);
            }
        }
    }
    reached
}

/// The parsed `package.json` of a directory of the checkout (`""` is the root): `None` when there is none, `Null` when
/// it cannot be read or parsed.
type ManifestReader<'a> = dyn Fn(&str) -> Option<Rc<Value>> + 'a;

/// Whether a file exists, by its path relative to the checkout with `/` separators.
type FileProbe<'a> = dyn Fn(&str) -> bool + 'a;

/// The build directories of a package directory of the checkout (`""` is the root), `(out_dir, root_dir)` relative to
/// it, in the order skif reads them ([`build_dir_reader`]).
type BuildDirReader<'a> = dyn Fn(&str) -> Vec<(String, String)> + 'a;

/// A [`BuildDirReader`] for a checkout whose packages declare no build directory.
fn no_build_dirs(_: &str) -> Vec<(String, String)> {
    Vec::new()
}

/// What the classification reads of a checkout besides the two reports: knip's workspaces, the manifests, the build
/// directories and which files exist.
struct Layout<'a> {
    /// The workspace member directories, relative with `/` separators; the root is a workspace without being listed.
    members: BTreeSet<String>,
    manifest: &'a ManifestReader<'a>,
    is_file: &'a FileProbe<'a>,
    build_dirs: &'a BuildDirReader<'a>,
    /// [`DEFAULT_ENTRY`], compiled as skif compiles package-relative globs.
    default_entry: IncompleteScopeGlobs,
}

impl<'a> Layout<'a> {
    fn new(members: BTreeSet<String>, manifest: &'a ManifestReader<'a>, is_file: &'a FileProbe<'a>) -> Self {
        Self {
            members,
            manifest,
            is_file,
            build_dirs: &no_build_dirs,
            default_entry: IncompleteScopeGlobs::new(DEFAULT_ENTRY.iter().map(|glob| (*glob).to_owned()).collect()),
        }
    }

    /// Whether `path` is a default entry file of its project ([`DEFAULT_ENTRY`], relative to the project directory).
    fn is_default_entry(&self, path: &str) -> bool {
        let dir = owner_dir(path, &|dir: &str| self.is_project(dir));
        self.default_entry.is_match(below(&dir, path))
    }

    /// Whether skif makes `path` an entry of its project from the manifest and the defaults alone: a default entry file
    /// or the source of a published path ([`Self::publishes_source_of`]). Entries from plugins and configuration are
    /// not known here.
    fn is_entry_module(&self, path: &str) -> bool {
        self.is_default_entry(path) || self.publishes_source_of(path)
    }

    /// The same layout, with the build directories of each package from `build_dirs`.
    fn with_build_dirs(self, build_dirs: &'a BuildDirReader<'a>) -> Self {
        Self { build_dirs, ..self }
    }

    /// Whether `dir` holds a project manifest, as skif's file discovery decides it: an unreadable manifest or one with
    /// a field besides [`MARKER_FIELDS`]. A marker-only manifest (`{"type": "module"}`) belongs to the package above
    /// it, so it is never the owner of a finding.
    fn is_project(&self, dir: &str) -> bool {
        (self.manifest)(dir).is_some_and(|manifest| {
            manifest
                .as_object()
                .is_none_or(|fields| fields.keys().any(|key| !MARKER_FIELDS.contains(&key.as_str())))
        })
    }

    /// Whether `dir` is one of knip's workspaces: the root or a member.
    fn is_workspace(&self, dir: &str) -> bool {
        dir.is_empty() || self.members.contains(dir)
    }

    /// The directory of the deepest project manifest above `path` when it is not one of knip's workspaces.
    fn non_member_project(&self, path: &str) -> Option<String> {
        let dir = owner_dir(path, &|dir: &str| self.is_project(dir));
        (!self.is_workspace(&dir)).then_some(dir)
    }

    /// Whether the manifest of `dir` lists `name` in one of its [`DEPENDENCY_FIELDS`].
    fn lists(&self, dir: &str, name: &str) -> bool {
        (self.manifest)(dir).is_some_and(|manifest| {
            DEPENDENCY_FIELDS.iter().any(|field| {
                manifest
                    .get(*field)
                    .and_then(Value::as_object)
                    .is_some_and(|dependencies| dependencies.contains_key(name))
            })
        })
    }

    /// Whether a directory of `path` is one skif's file discovery skips by name: [`SKIPPED_DIRECTORIES`] (those of
    /// [`PACKAGE_ROOT_ONLY_DIRECTORIES`] directly in the root or a package directory) and hidden directories other
    /// than [`TOOL_DIRECTORIES`], unless the directory is a workspace member. A skipped directory that holds members
    /// (`.vscode/extensions/*`) is walked for those members only, so a file there counts as skipped unless it lies
    /// in one of them.
    fn in_skipped_directory(&self, path: &str) -> bool {
        let Some((directories, _)) = path.rsplit_once('/') else {
            return false;
        };
        let mut parent = "";
        let mut start = 0;
        for end in directories
            .match_indices('/')
            .map(|(at, _)| at)
            .chain([directories.len()])
        {
            let name = &directories[start..end];
            let dir = &directories[..end];
            let by_name =
                SKIPPED_DIRECTORIES.contains(&name) || (name.starts_with('.') && !TOOL_DIRECTORIES.contains(&name));
            let here = !PACKAGE_ROOT_ONLY_DIRECTORIES.contains(&name) || parent.is_empty() || self.is_project(parent);
            if by_name && here && !self.members.contains(dir) && !self.in_member_below(dir, path) {
                return true;
            }
            parent = dir;
            start = end + 1;
        }
        false
    }

    /// Whether `path` lies in a workspace member below the directory `dir`.
    fn in_member_below(&self, dir: &str, path: &str) -> bool {
        let prefix = format!("{dir}/");
        self.members
            .iter()
            .any(|member| member.starts_with(&prefix) && path.starts_with(&format!("{member}/")))
    }

    /// Whether `path` is a TypeScript file whose package publishes its path with a built extension: a published path
    /// with `*` whose pattern it matches ([`is_published_twin`]), or one without `*` that skif maps to it.
    fn publishes_source_of(&self, path: &str) -> bool {
        let Some((stem, extension)) = typescript_source(path) else {
            return false;
        };
        let dir = owner_dir(path, &|dir: &str| self.is_project(dir));
        let Some(manifest) = (self.manifest)(&dir) else {
            return false;
        };
        let relative = if dir.is_empty() {
            Some(stem)
        } else {
            stem.strip_prefix(dir.as_str()).and_then(|rest| rest.strip_prefix('/'))
        };
        relative.is_some_and(|stem| {
            let source = format!("{stem}.{extension}");
            published_paths(&manifest).iter().any(|target| {
                if target.contains('*') {
                    is_published_twin(target, stem, extension)
                } else {
                    self.source_of(&dir, target).is_some_and(|found| found == source)
                }
            })
        })
    }

    /// The source, relative to the package directory `dir`, that skif maps the published path `target` (without `*`)
    /// to, as `map_published` in `skif-js` picks it: through the package's build directories first (a path below an
    /// `outDir` names the same path below its `rootDir`, [`build_source`]), then by the `dist` to `src` convention,
    /// each time the first of `skif_js::source_candidates` that exists.
    fn source_of(&self, dir: &str, target: &str) -> Option<String> {
        let first = |published: &str| {
            source_candidates(published).into_iter().find(|candidate| {
                let path = if dir.is_empty() {
                    candidate.clone()
                } else {
                    format!("{dir}/{candidate}")
                };
                (self.is_file)(&path)
            })
        };
        let clean = join_relative("", target).unwrap_or_default();
        (self.build_dirs)(dir)
            .iter()
            .filter_map(|(out_dir, root_dir)| build_source(out_dir, root_dir, &clean))
            .find_map(|source| first(&source))
            .or_else(|| first(target))
    }
}

/// The source path behind `rel`, a package-relative path inside the build output directory `out_dir`, below its
/// `root_dir` (`source_paths` in `skif-js`); `None` for a path outside `out_dir`.
fn build_source(out_dir: &str, root_dir: &str, rel: &str) -> Option<String> {
    if out_dir.is_empty() {
        return None;
    }
    let rest = rel.strip_prefix(out_dir)?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    Some(match (root_dir, rest.trim_start_matches('/')) {
        ("", rest) => rest.to_owned(),
        (root, "") => root.to_owned(),
        (root, rest) => format!("{root}/{rest}"),
    })
}

/// Whether `path` names a declaration file (`.d.ts`, `.d.mts`, `.d.cts`).
fn is_declaration(path: &str) -> bool {
    [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
}

/// Whether the last segment of an `include` pattern names a file: it has a stem and a code extension.
fn has_input_extension(segment: &str) -> bool {
    segment.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty() && CODE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// The `rootDir` TypeScript uses when a `tsconfig` sets none, as skif's TypeScript plugin approximates it: the longest
/// common directory of the `include` patterns (each up to its first wildcard, a file pattern without its name) and the
/// directories of the `files`, declaration files left out. `None` without either, or when one leaves the directory.
fn inferred_root_dir(config: &Value) -> Option<String> {
    let strings = |field: &str| {
        let mut found = Vec::new();
        if let Some(value) = config.get(field) {
            collect_strings(value, &mut found);
        }
        found
    };
    let mut roots = Vec::new();
    for pattern in strings("include").iter().filter(|pattern| !is_declaration(pattern)) {
        let segments: Vec<&str> = pattern.split('/').collect();
        let directory = match segments
            .iter()
            .position(|segment| segment.contains(['*', '?', '{', '[']))
        {
            Some(at) => &segments[..at],
            None if segments.last().is_some_and(|last| has_input_extension(last)) => &segments[..segments.len() - 1],
            None => &segments[..],
        };
        roots.push(join_relative("", &directory.join("/"))?);
    }
    for file in strings("files").iter().filter(|file| !is_declaration(file)) {
        roots.push(join_relative("", parent_dir(file))?);
    }
    let (first, rest) = roots.split_first()?;
    let mut common: Vec<&str> = first.split('/').filter(|segment| !segment.is_empty()).collect();
    for root in rest {
        let shared = common
            .iter()
            .zip(root.split('/').filter(|segment| !segment.is_empty()))
            .take_while(|&(ours, theirs)| *ours == theirs)
            .count();
        common.truncate(shared);
    }
    Some(common.join("/"))
}

/// The build directory a `tsconfig` declares, `(out_dir, root_dir)` relative to its directory, as skif's TypeScript
/// plugin reads it: `compilerOptions.outDir` with `rootDir`, or without one the root [`inferred_root_dir`] gives.
/// `None` without `outDir`, without a root, for a path that leaves the directory, or for a file that is not JSON with
/// comments.
fn tsconfig_build_dir(text: &str) -> Option<(String, String)> {
    let config: Value = serde_json::from_str(&strip_jsonc(text)).ok()?;
    let options = config.get("compilerOptions")?;
    let out_dir = join_relative("", options.get("outDir")?.as_str()?)?;
    let root_dir = match options.get("rootDir").and_then(Value::as_str) {
        Some(root_dir) => join_relative("", root_dir)?,
        None => inferred_root_dir(&config)?,
    };
    (!out_dir.is_empty()).then_some((out_dir, root_dir))
}

/// Reads the build directories of the package directories of a checkout, each once: those of the `tsconfig.json`,
/// `tsconfig.*.json` and `jsconfig.json` files directly in the directory ([`tsconfig_build_dir`]), in file name order.
/// A configuration another one extends from elsewhere is not read.
fn build_dir_reader(root: &Path) -> impl Fn(&str) -> Vec<(String, String)> + '_ {
    let cache: RefCell<BTreeMap<String, Vec<(String, String)>>> = RefCell::default();
    move |dir: &str| {
        if let Some(known) = cache.borrow().get(dir) {
            return known.clone();
        }
        let package = root.join(dir);
        let names: BTreeSet<String> = fs::read_dir(&package)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name == "jsconfig.json" || (name.starts_with("tsconfig.") && name.ends_with(".json")))
            .collect();
        let found: Vec<(String, String)> = names
            .iter()
            .filter_map(|name| fs::read_to_string(package.join(name)).ok())
            .filter_map(|text| tsconfig_build_dir(&text))
            .collect();
        cache.borrow_mut().insert(dir.to_owned(), found.clone());
        found
    }
}

/// Decides which known deviation, if any, a one-sided finding is.
struct Classifier<'a> {
    knip_unread: &'a [String],
    include_test_files: bool,
    /// The packages skif did not judge and what it left unjudged in each (`not_judged_dirs`).
    not_judged: &'a BTreeMap<String, NotJudgedScope>,
    /// The packages holding a configuration knip could not load.
    knip_failed: &'a BTreeSet<String>,
    /// The `package.json` files knip reports any finding on.
    knip_manifests: &'a BTreeSet<String>,
    /// The `unlisted` keys whose import only another package's tsconfig `paths` map (ADR 0075).
    foreign_paths: &'a BTreeSet<String>,
    /// The files knip reports as unused, before any are set apart.
    knip_unused_files: &'a BTreeSet<String>,
    /// The files skif reports as unused.
    skif_unused_files: &'a BTreeSet<String>,
    /// `<manifest>|<package>` for every package a file only knip lists as unused imports, with the manifest of the
    /// file's workspace (`unused_file_imports`).
    knip_unused_imports: &'a BTreeSet<String>,
    /// The same for the files only skif reports as unused.
    skif_unused_imports: &'a BTreeSet<String>,
    /// The package directories of the roots of a recursive skif run (`skif_root_packages`).
    skif_root_packages: &'a BTreeSet<String>,
    /// The files only knip reports as unused that a published source reaches (`published_reach`).
    published_reach: &'a BTreeSet<String>,
    /// `<file>|<export>` for every export that a file only knip lists as unused imports (`unused_file_credits`).
    knip_unused_credits: &'a BTreeSet<String>,
    /// The same for the files only skif reports as unused.
    skif_unused_credits: &'a BTreeSet<String>,
    /// `<manifest>|<package>` for every dependency a `BUILD` file credits (`bazel_credits`).
    bazel_credits: &'a BTreeSet<String>,
    /// knip ran with the scratch root manifest of a Rush repository (`knip-root = "rush"`).
    rush_root: bool,
    layout: &'a Layout<'a>,
}

impl Classifier<'_> {
    fn classify(&self, side: Side, category: &str, key: &str) -> Option<&'static str> {
        if self.knip_unread.iter().any(|prefix| key.starts_with(prefix.as_str())) {
            return Some(KNIP_UNREAD);
        }
        let path = key_path(key);
        let is_package = |dir: &str| self.layout.is_project(dir);
        // A configuration knip could not load changes what it reads in the whole package, so both sides are affected.
        let config_error = || {
            self.knip_failed
                .contains(&owner_dir(key, &is_package))
                .then_some(KNIP_CONFIG_ERROR)
        };
        if side == Side::Skif {
            return skif_only_deviation(category, key, self.foreign_paths)
                .or_else(|| self.unjudged_manifest(category, path).then_some(KNIP_UNJUDGED_MANIFEST))
                .or_else(|| self.skif_non_member_project(category, path).then_some(NON_MEMBER_PROJECT))
                .or_else(|| (category == "unlisted" && is_build_file(path)).then_some(BAZEL_BUILD_FILE))
                .or_else(|| in_unused_file(category, path, self.knip_unused_files).then_some(IN_KNIP_UNUSED_FILE))
                .or_else(|| {
                    used_in_unused_file(self.skif_unused_imports, category, key).then_some(USED_IN_SKIF_UNUSED_FILE)
                })
                .or_else(|| {
                    imported_by_unused_file(self.skif_unused_credits, category, key)
                        .then_some(IMPORTED_BY_SKIF_UNUSED_FILE)
                })
                .or_else(config_error);
        }
        (category == "files")
            .then(|| classify_file(key, self.include_test_files))
            .flatten()
            .or_else(|| {
                (category != "files" && FIXTURE_CATEGORIES.contains(&category) && in_fixture_directory(path))
                    .then_some(FIXTURE_FILE)
            })
            .or_else(|| (self.rush_root && owner_dir(key, &is_package).is_empty()).then_some(RUSH_ROOT_FILE))
            .or_else(|| not_judged(category, key, self.not_judged, &is_package).then_some(NOT_JUDGED))
            .or_else(|| self.layout.in_skipped_directory(path).then_some(SKIPPED_DIRECTORY))
            .or_else(|| self.non_member_project(category, key).then_some(NON_MEMBER_PROJECT))
            .or_else(|| self.published_source(category, path).then_some(PUBLISHED_SOURCE))
            .or_else(|| {
                (category == "duplicates" && self.layout.is_entry_module(path)).then_some(ENTRY_MODULE_DUPLICATE)
            })
            .or_else(|| in_unused_file(category, path, self.skif_unused_files).then_some(IN_SKIF_UNUSED_FILE))
            .or_else(|| {
                used_in_unused_file(self.knip_unused_imports, category, key).then_some(USED_IN_KNIP_UNUSED_FILE)
            })
            .or_else(|| {
                (IMPORTED_CATEGORIES.contains(&category) && self.bazel_credits.contains(key)).then_some(BAZEL_BUILD_FILE)
            })
            .or_else(|| {
                imported_by_unused_file(self.knip_unused_credits, category, key).then_some(IMPORTED_BY_KNIP_UNUSED_FILE)
            })
            .or_else(config_error)
    }

    /// Whether a finding only knip reports follows from a published source being an entry of skif: an unused file that
    /// is a published source or one a published source reaches, or an export or type ([`EXPORT_LABELS`]) of a
    /// published source, which is public as an entry's export.
    fn published_source(&self, category: &str, path: &str) -> bool {
        if category == "files" {
            return self.layout.publishes_source_of(path) || self.published_reach.contains(path);
        }
        EXPORT_LABELS.contains(&category) && self.layout.publishes_source_of(path)
    }

    /// Whether a finding only skif reports sits on a `package.json` that is not one of knip's workspaces.
    fn unjudged_manifest(&self, category: &str, path: &str) -> bool {
        MANIFEST_CATEGORIES.contains(&category)
            && path
                .strip_suffix("/package.json")
                .is_some_and(|dir| !self.layout.is_workspace(dir))
            && !self.knip_manifests.contains(path)
    }

    /// Whether a finding only skif reports in [`NON_MEMBER_SKIF_CATEGORIES`] lies below a project directory that is not
    /// one of knip's workspaces; a binary on a `package.json` that knip reports anything on stays compared.
    fn skif_non_member_project(&self, category: &str, path: &str) -> bool {
        NON_MEMBER_SKIF_CATEGORIES.contains(&category)
            && !self.knip_manifests.contains(path)
            && self.layout.non_member_project(path).is_some()
    }

    /// Whether a finding only knip reports in a file (any category but the manifest ones) lies below a project
    /// directory that is not one of knip's workspaces; an unlisted dependency only when that project's own manifest
    /// lists it, since skif judges the project's imports against that manifest. In a project that a recursive skif run
    /// lints as a package of one of its roots, [`ROOT_COMPARED_CATEGORIES`] stay compared, except the exports and types
    /// of the project's default entry files, which are public there while knip reads the files without the project's
    /// entries.
    fn non_member_project(&self, category: &str, key: &str) -> bool {
        if MANIFEST_CATEGORIES.contains(&category) {
            return false;
        }
        let Some(dir) = self.layout.non_member_project(key_path(key)) else {
            return false;
        };
        if ROOT_COMPARED_CATEGORIES.contains(&category) && self.skif_root_packages.contains(&dir) {
            return EXPORT_LABELS.contains(&category) && self.layout.is_default_entry(key_path(key));
        }
        category != "unlisted"
            || key
                .split_once('|')
                .is_some_and(|(_, name)| self.layout.lists(&dir, package_name(name)))
    }
}

/// Whether a finding inside a file (any category but `files` and the manifest ones) sits in a file the other tool
/// reports as unused: a tool analyses nothing inside a file it calls unused, so such a finding follows from the `files`
/// disagreement, which is counted once already. A dependency knip reports unused because only such a file imports it is
/// `used-in-knip-unused-file` (or `used-in-skif-unused-file`) instead.
fn in_unused_file(category: &str, path: &str, unused: &BTreeSet<String>) -> bool {
    category != "files" && !MANIFEST_CATEGORIES.contains(&category) && unused.contains(path)
}

/// Whether a dependency finding (`<manifest>|<package>`) names a package that `imports` (from [`unused_file_imports`])
/// holds for that manifest: one a file of its workspace that only one tool calls unused imports; an `@types/` package
/// also when that file imports its runtime package.
fn used_in_unused_file(imports: &BTreeSet<String>, category: &str, key: &str) -> bool {
    if !IMPORTED_CATEGORIES.contains(&category) {
        return false;
    }
    let Some((manifest, name)) = key.split_once('|') else {
        return false;
    };
    imports.contains(key)
        || name
            .strip_prefix("@types/")
            .is_some_and(|typed| imports.contains(&format!("{manifest}|{}", typed_package(typed))))
}

/// The package an `@types/` package types: `node` for `@types/node`, `@scope/name` for `@types/scope__name`.
fn typed_package(typed: &str) -> String {
    typed
        .split_once("__")
        .map_or_else(|| typed.to_owned(), |(scope, name)| format!("@{scope}/{name}"))
}

/// The package a module specifier names when it is a bare specifier: not relative, absolute, a subpath import (`#x`)
/// or a URL or protocol (`node:fs`, `data:`, `https://`).
fn bare_package(specifier: &str) -> Option<&str> {
    let first = specifier.split('/').next().unwrap_or(specifier);
    let local = specifier.is_empty() || specifier.starts_with(['.', '/', '#', '\\']) || first.contains(':');
    (!local).then(|| package_name(specifier))
}

/// The packages a source file imports by bare specifier, as skif's parser reads its imports, requires, re-exports and
/// dynamic imports; globs and test-runner mocks are not imports of a package. Empty for a file it cannot parse.
fn imported_packages(path: &str, source: &str) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let Ok(module) = parse_module(Path::new(path), source, &options) else {
        return BTreeSet::new();
    };
    module
        .imports
        .iter()
        .filter(|import| {
            !matches!(
                import.kind,
                ImportKind::ImportMetaGlob | ImportKind::SourceGlob | ImportKind::Mock
            )
        })
        .filter_map(|import| bare_package(&import.specifier))
        .map(str::to_owned)
        .collect()
}

/// `<manifest>|<package>` for every package that one of the files only one tool lists as unused (`unused` but not
/// `other_unused`) imports, where `<manifest>` is the `package.json` of the file's workspace (`workspace_of`): what
/// that tool would have credited had it analysed the file, and the other tool did credit. A file both tools call
/// unused credits nothing on either side, so a dependency only it imports is a plain disagreement. `read` gives a
/// file's source; a file it cannot read imports nothing.
fn unused_file_imports(
    unused: &BTreeSet<String>,
    other_unused: &BTreeSet<String>,
    read: &dyn Fn(&str) -> Option<String>,
    workspace_of: &dyn Fn(&str) -> String,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for file in unused.difference(other_unused) {
        let Some(source) = read(file) else { continue };
        let workspace = workspace_of(file);
        let manifest = if workspace.is_empty() {
            "package.json".to_owned()
        } else {
            format!("{workspace}/package.json")
        };
        keys.extend(
            imported_packages(file, &source)
                .into_iter()
                .map(|package| format!("{manifest}|{package}")),
        );
    }
    keys
}

/// Whether `path` is a `BUILD.bazel` or `BUILD` file.
fn is_build_file(path: &str) -> bool {
    BUILD_FILES.contains(&path.rsplit('/').next().unwrap_or(path))
}

/// The contents of the string literals of Starlark `source`, in order, without those of `#` comments: single, double
/// and triple quoted, an escaped character taken as itself; a literal that is not closed ends with its line.
fn starlark_strings(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(&character) = chars.get(at) {
        match character {
            '#' => {
                while chars.get(at).is_some_and(|&next| next != '\n') {
                    at += 1;
                }
            }
            '"' | '\'' => {
                let quote = character;
                let triple = chars.get(at + 1) == Some(&quote) && chars.get(at + 2) == Some(&quote);
                let width = if triple { 3 } else { 1 };
                at += width;
                let mut text = String::new();
                while let Some(&next) = chars.get(at) {
                    if next == quote
                        && (!triple || (chars.get(at + 1) == Some(&quote) && chars.get(at + 2) == Some(&quote)))
                    {
                        at += width;
                        break;
                    }
                    if next == '\n' && !triple {
                        break;
                    }
                    if next == '\\'
                        && let Some(&escaped) = chars.get(at + 1)
                    {
                        text.push(escaped);
                        at += 2;
                        continue;
                    }
                    text.push(next);
                    at += 1;
                }
                found.push(text);
            }
            _ => at += 1,
        }
    }
    found
}

/// The npm package a path below `node_modules` starts with (`@scope/name` or `name`); `None` for an empty name or a
/// scope without one.
fn npm_package_name(path: &str) -> Option<&str> {
    let name = package_name(path);
    let valid = match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, rest)| !scope.is_empty() && !rest.is_empty()),
        None => !name.is_empty(),
    };
    valid.then_some(name)
}

/// The Bazel package, relative to its repository root, and the npm package that `label` names as a linked dependency
/// when a `BUILD` file of the Bazel package `bazel_package` holds it (`linked_dependency` of skif's Bazel plugin): a
/// `node_modules/<name>` target of the main repository (`":node_modules/x"`, `"//pkg:node_modules/@scope/x"`,
/// `"@//pkg:node_modules/x"`) or the `<name>/package_json.bzl` of another repository (`"@npm//pkg:x/package_json.bzl"`).
fn linked_dependency(label: &str, bazel_package: &str) -> Option<(String, String)> {
    let (repository, rest) = match label.strip_prefix('@') {
        Some(after) => {
            let at = after.find("//")?;
            (after[..at].trim_start_matches('@'), &after[at..])
        }
        None => ("", label),
    };
    let (package, target) = match rest.strip_prefix("//") {
        Some(absolute) => absolute.split_once(':')?,
        None => (bazel_package, rest.strip_prefix(':').unwrap_or(rest)),
    };
    if let Some(name) = target.strip_suffix("/package_json.bzl") {
        let named = !repository.is_empty() && npm_package_name(name) == Some(name);
        return named.then(|| (package.to_owned(), name.to_owned()));
    }
    let linked = target.strip_prefix("node_modules/")?;
    if !repository.is_empty() {
        return None;
    }
    npm_package_name(linked).map(|name| (package.to_owned(), name.to_owned()))
}

/// The root of the Bazel repository that holds the directory `dir`: the nearest directory at or above it with one of
/// [`BAZEL_ROOT_FILES`], up to the checkout root.
fn bazel_root<'p>(dir: &'p str, is_file: &FileProbe<'_>) -> Option<&'p str> {
    let mut candidate = dir;
    loop {
        if BAZEL_ROOT_FILES
            .iter()
            .any(|name| is_file(&joined(candidate, name)))
        {
            return Some(candidate);
        }
        if candidate.is_empty() {
            return None;
        }
        candidate = parent_dir(candidate);
    }
}

/// `<manifest>|<package>` for every dependency a `BUILD.bazel` or `BUILD` file credits, as skif's Bazel plugin reads
/// it: a string literal that is a linked-dependency label ([`linked_dependency`]) names a dependency of the package
/// whose directory is the label's Bazel package, below the file's Bazel repository root ([`bazel_root`], one whose
/// root files name [`RULES_JS`]), when that directory holds a `package.json` and the `BUILD` file lies in it.
/// `build_files` holds each file's path and source; `read` gives the source of a root file.
fn bazel_credits(
    build_files: &[(String, String)],
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let mut uses_rules_js: BTreeMap<String, bool> = BTreeMap::new();
    let mut credits = BTreeSet::new();
    for (path, source) in build_files {
        let dir = parent_dir(path);
        let Some(root) = bazel_root(dir, is_file) else {
            continue;
        };
        let named = *uses_rules_js.entry(root.to_owned()).or_insert_with(|| {
            BAZEL_ROOT_FILES
                .iter()
                .any(|name| read(&joined(root, name)).is_some_and(|text| text.contains(RULES_JS)))
        });
        if !named {
            continue;
        }
        let bazel_package = below(root, dir);
        for literal in starlark_strings(source) {
            let Some((package, name)) = linked_dependency(&literal, bazel_package) else {
                continue;
            };
            let package_dir = joined(root, &package);
            let manifest = joined(&package_dir, "package.json");
            let inside = package_dir.is_empty() || dir == package_dir || dir.starts_with(&format!("{package_dir}/"));
            if inside && is_file(&manifest) {
                credits.insert(format!("{manifest}|{name}"));
            }
        }
    }
    credits
}

/// The `BUILD.bazel` and `BUILD` files of a checkout whose root holds one of [`BAZEL_ROOT_FILES`], with their sources,
/// by path relative to the checkout with `/` separators, sorted: in the directories skif's file discovery enters (none
/// of [`SKIPPED_DIRECTORIES`], no hidden directory but [`TOOL_DIRECTORIES`]), never through a symbolic link or into
/// Bazel's `bazel-*` output trees. Empty for a checkout that is no Bazel repository.
fn read_build_files(root: &Path) -> Vec<(String, String)> {
    if !BAZEL_ROOT_FILES.iter().any(|name| root.join(name).is_file()) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(root.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let (Ok(name), Ok(kind)) = (entry.file_name().into_string(), entry.file_type()) else {
                continue;
            };
            let path = joined(&dir, &name);
            if kind.is_dir() {
                let skipped = SKIPPED_DIRECTORIES.contains(&name.as_str())
                    || (name.starts_with('.') && !TOOL_DIRECTORIES.contains(&name.as_str()))
                    || (dir.is_empty() && name.starts_with("bazel-"));
                if !skipped {
                    pending.push(path);
                }
            } else if kind.is_file()
                && BUILD_FILES.contains(&name.as_str())
                && let Ok(source) = fs::read_to_string(root.join(&path))
            {
                found.push((path, source));
            }
        }
    }
    found.sort();
    found
}

/// The `package.json` files knip reports a finding on.
fn knip_manifests(knip: &Keys) -> BTreeSet<String> {
    knip.values()
        .flatten()
        .map(|key| key_path(key.as_str()))
        .filter(|path| *path == "package.json" || path.ends_with("/package.json"))
        .map(str::to_owned)
        .collect()
}

/// Reads the manifests of a checkout, each once.
fn manifest_reader(root: &Path) -> impl Fn(&str) -> Option<Rc<Value>> + '_ {
    let cache: RefCell<BTreeMap<String, Option<Rc<Value>>>> = RefCell::default();
    move |dir: &str| {
        if let Some(known) = cache.borrow().get(dir) {
            return known.clone();
        }
        let path = root.join(dir).join("package.json");
        let manifest = path.is_file().then(|| {
            Rc::new(
                fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(Value::Null),
            )
        });
        cache.borrow_mut().insert(dir.to_owned(), manifest.clone());
        manifest
    }
}

/// The workspace member directories of a checkout as skif's workspace discovery reads the root's patterns, relative
/// with `/` separators, the root left out.
fn workspace_members(root: &Path) -> BTreeSet<String> {
    declared_member_dirs(&OsFs, root)
        .iter()
        .filter_map(|dir| dir.strip_prefix(root).ok())
        .map(|dir| dir.to_string_lossy().replace('\\', "/").trim_matches('/').to_owned())
        .filter(|dir| !dir.is_empty() && dir != ".")
        .collect()
}

/// The knip report in its stdout, which may carry banners or log lines around the JSON (dotenv, console output of
/// config files). Tries every `{` that starts a line or a string key, and accepts the first object that is a report.
pub fn parse_knip_report(stdout: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(stdout);
    if text.trim().is_empty() {
        return Err("stdout is empty".to_owned());
    }
    let bytes = text.as_bytes();
    let (line_starts, inline): (Vec<usize>, Vec<usize>) = text
        .match_indices('{')
        .map(|(at, _)| at)
        .filter(|&at| at == 0 || bytes[at - 1] == b'\n' || bytes.get(at + 1) == Some(&b'"'))
        .partition(|&at| at == 0 || bytes[at - 1] == b'\n');
    let mut first_error = None;
    for start in line_starts.into_iter().chain(inline.into_iter().take(256)) {
        match serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Value>()
            .next()
        {
            Some(Ok(document))
                if document.get("issues").is_some_and(Value::is_array)
                    || document.get("files").is_some_and(Value::is_array) =>
            {
                return Ok(document);
            }
            Some(Err(error)) => {
                first_error.get_or_insert_with(|| error.to_string());
            }
            _ => {}
        }
    }
    Err(first_error.map_or_else(
        || format!("no knip report in the {} bytes of stdout", stdout.len()),
        |error| format!("no knip report in stdout; first parse error: {error}"),
    ))
}

/// The configuration files knip could not load, from its `ERROR: Error loading <file> (<cause>)` lines on stderr,
/// relative to the checkout. knip goes on without them and still prints a report, then exits with 2.
pub fn knip_config_errors(stderr: &str, root: &Path) -> BTreeSet<String> {
    stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ERROR: Error loading "))
        .filter_map(|rest| rest.split(" (").next())
        .map(|file| normalize(file.trim(), root))
        .filter(|file| !file.is_empty())
        .collect()
}

fn total(keys: &Keys) -> usize {
    keys.values().map(std::collections::BTreeSet::len).sum()
}

/// The install command of a repository (program first), or why there is none.
fn install_command(repo: &Repo, dir: &Path) -> Result<(String, Vec<String>), String> {
    if !repo.install {
        return Err(format!("install is disabled for this repository: {}", repo.notes));
    }
    if let Some((program, args)) = repo.install_command.split_first() {
        return Ok((program.clone(), args.to_vec()));
    }
    let (program, args): (&str, &[&str]) = match repo.package_manager.as_str() {
        "pnpm" => ("pnpm", &["install", "--ignore-scripts", "--frozen-lockfile"]),
        "npm" => ("npm", &["ci", "--ignore-scripts"]),
        "bun" => ("bun", &["install", "--ignore-scripts", "--frozen-lockfile"]),
        "yarn" if dir.join(".yarnrc.yml").is_file() => ("yarn", &["install", "--immutable", "--mode=skip-build"]),
        "yarn" => ("yarn", &["install", "--frozen-lockfile", "--ignore-scripts"]),
        other => return Err(format!("no install step for package manager {other}")),
    };
    Ok((program.to_owned(), args.iter().map(|a| (*a).to_owned()).collect()))
}

/// Installs the dependencies of a checkout without running lifecycle scripts, then runs the repository's
/// `build-command` if it has one, within `build-timeout-secs` (the install's time limit when unset); `Some(reason)` when
/// either fails.
///
/// # Errors
/// Fails when a command cannot be started or the marker cannot be written.
pub fn install(repo: &Repo, dir: &Path, scratch: &Path, default_timeout: Duration) -> Result<Option<String>> {
    let timeout = repo.install_timeout_secs.map_or(default_timeout, Duration::from_secs);
    let build_timeout = repo.build_timeout_secs.map_or(timeout, Duration::from_secs);
    let marker = dir.join(INSTALLED_MARKER);
    if is_installed(repo, dir) {
        return Ok(None);
    }
    let (program, args) = match install_command(repo, dir) {
        Ok(command) => command,
        Err(reason) => return Ok(Some(reason)),
    };
    if let Some(reason) = run_step(repo, dir, scratch, ("install", &program, &args), timeout)? {
        return Ok(Some(reason));
    }
    if let Some((program, args)) = repo.build_command.split_first()
        && let Some(reason) = run_step(repo, dir, scratch, ("build", program, args), build_timeout)?
    {
        return Ok(Some(reason));
    }
    fs::write(&marker, install_marker(repo)).with_context(|| format!("writing {}", marker.display()))?;
    Ok(None)
}

/// Runs one step (`install` or `build`: its name, program and arguments) of the install in the checkout, with its
/// output captured in the scratch directory under the step's name; `Some(reason)` when it cannot run or fails.
fn run_step(
    repo: &Repo,
    dir: &Path,
    scratch: &Path,
    (step, program, args): (&str, &str, &[String]),
    timeout: Duration,
) -> Result<Option<String>> {
    if find_on_path(program).is_none() {
        return Ok(Some(format!("{program} is not installed")));
    }
    eprintln!("parity: {} {program} {}", repo.name, args.join(" "));
    let run = capture(
        Command::new(program)
            .args(args)
            .current_dir(dir)
            .env("CI", "1")
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        scratch,
        step,
        timeout,
    )?;
    if run.timed_out {
        return Ok(Some(format!(
            "{program} {step} timed out after {} s",
            timeout.as_secs()
        )));
    }
    if run.code != Some(0) {
        return Ok(Some(format!("{program} {step} failed: {}", run.failure_summary())));
    }
    Ok(None)
}

/// What the install marker of a checkout holds once the install (and the build, if any) succeeded: the pinned commit,
/// then a line for the corpus `install-command` and one for the `build-command` when the repository sets them, so a
/// marker written before a command was added or changed is stale.
fn install_marker(repo: &Repo) -> String {
    let mut marker = repo.rev.clone();
    for (label, command) in [("install", &repo.install_command), ("build", &repo.build_command)] {
        if !command.is_empty() {
            let _ = write!(marker, "\n{label}: {}", command.join(" "));
        }
    }
    marker
}

/// Whether the dependencies of a checkout are installed at its pinned commit: its install marker holds the commit
/// (and the build command, see [`install_marker`]).
pub fn is_installed(repo: &Repo, dir: &Path) -> bool {
    fs::read_to_string(dir.join(INSTALLED_MARKER)).is_ok_and(|marker| marker.trim() == install_marker(repo))
}

/// How many of the compared repositories were installed: `yes`, `no` or `partial (n of m)`.
fn installed_summary(entries: &BTreeMap<String, Entry>) -> String {
    let compared: Vec<&Entry> = entries.values().filter(|entry| entry.status == COMPARED).collect();
    let installed = compared.iter().filter(|entry| entry.installed == Some(true)).count();
    match installed {
        0 => "no".to_owned(),
        n if n == compared.len() => "yes".to_owned(),
        n => format!("partial ({n} of {})", compared.len()),
    }
}

struct Outcome {
    entry: Entry,
    detail: Value,
}

fn skipped(reason: impl Into<String>) -> Outcome {
    let reason = reason.into();
    Outcome {
        detail: json!({ "status": diff::SKIPPED, "reason": reason }),
        entry: Entry::skipped(reason),
    }
}

/// The categories the comparison counts and the known deviations it sets apart, as the baseline records them.
fn category_set() -> CategorySet {
    let compared: BTreeSet<String> = KNIP_CATEGORIES
        .iter()
        .map(|(_, category)| (*category).to_owned())
        .collect();
    CategorySet {
        compared: compared.into_iter().collect(),
        known_deviations: KNIP_DEVIATIONS
            .iter()
            .map(|deviation| deviation.id.to_owned())
            .collect(),
    }
}

/// The unused files of one side of the comparison (its `files` findings).
fn unused_files(keys: &Keys) -> &BTreeSet<String> {
    static NONE: BTreeSet<String> = BTreeSet::new();
    keys.get("files").unwrap_or(&NONE)
}

/// skif's report on a checkout, and why its install or build failed, if it did.
struct SkifRun {
    doc: Value,
    run: Captured,
    recursive: bool,
    install_failure: Option<String>,
}

/// Installs the checkout when the run and the repository ask for it, then runs skif on it; `Ok(Err(reason))` when the
/// repository is skipped. A failed install or build does not skip it: both tools run on what the checkout holds,
/// which counts as not installed, and the failure is recorded with the run.
fn install_and_run_skif(
    options: &Options,
    skif: &Path,
    repo: &Repo,
    root: &Path,
    scratch: &Path,
) -> Result<Result<SkifRun, String>> {
    // A repository with `install = false` is compared uninstalled, so a tool that cannot run there says why itself.
    let install_failure = if options.install && repo.install {
        install(repo, root, scratch, options.timeout)?
    } else {
        None
    };
    if let Some(reason) = &install_failure {
        eprintln!("parity: {} is compared without a complete install: {reason}", repo.name);
    }
    let mut command = Command::new(skif);
    command
        .args([".", "--format", "json", "--no-install"])
        .current_dir(root);
    let recursive = options.recursive || repo.recursive;
    if recursive {
        command.arg("--recursive");
    }
    if !repo.entry.is_empty() {
        command.arg("--config").arg(write_config(
            scratch,
            "skif-entry.toml",
            &skif_entry_config(&repo.entry),
        )?);
    }
    let run = capture(&mut command, scratch, "skif-parity", options.timeout)?;
    if run.timed_out {
        return Ok(Err("skif timed out".to_owned()));
    }
    let doc: Value = match serde_json::from_slice(&run.stdout) {
        Ok(doc) => doc,
        Err(error) => return Ok(Err(format!("skif output is not JSON: {error}"))),
    };
    if doc.get("diagnostics").is_none() {
        let message = doc["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Ok(Err(format!("skif failed: {message}")));
    }
    Ok(Ok(SkifRun {
        doc,
        run,
        recursive,
        install_failure,
    }))
}

/// knip's report on a checkout, with knip's workspaces as they were while it ran.
struct KnipRun {
    doc: Value,
    run: Captured,
    /// The workspace member directories (`workspace_members`), read while the scratch root manifest that declares
    /// them for a Rush repository is there.
    members: BTreeSet<String>,
    /// knip ran with the scratch root manifest of a Rush repository (`knip-root = "rush"`).
    rush_root: bool,
}

/// Runs the pinned knip on a checkout, with the scratch root manifest (`knip_root`) there only while it runs;
/// `Ok(Err(reason))` when the repository is skipped.
fn run_knip(
    options: &Options,
    tools: &Tools,
    repo: &Repo,
    root: &Path,
    scratch: &Path,
) -> Result<Result<KnipRun, String>> {
    let scratch_root = match knip_root(repo, root) {
        Ok(manifest) => manifest,
        Err(reason) => return Ok(Err(format!("knip root: {reason}"))),
    };
    let spec = format!("knip@{}", tools.knip);
    let mut command = Command::new("npx");
    command
        .args(["--yes", spec.as_str(), "--reporter", "json"])
        .current_dir(root)
        .env("CI", "1")
        .env("DOTENV_CONFIG_QUIET", "true");
    if !options.repo_knip_config {
        let config = write_config(scratch, "knip-config.json", &knip_config(&repo.entry))?;
        command.arg("--config").arg(config);
    }
    let run = capture(&mut command, scratch, "knip", options.timeout)?;
    let members = workspace_members(root);
    let rush_root = scratch_root.is_some();
    drop(scratch_root);
    if run.timed_out {
        return Ok(Err("knip timed out".to_owned()));
    }
    match parse_knip_report(&run.stdout) {
        Ok(doc) => Ok(Ok(KnipRun {
            doc,
            run,
            members,
            rush_root,
        })),
        Err(reason) => Ok(Err(format!(
            "knip failed (exit {}): {reason}; stderr: {}",
            run.code.map_or_else(|| "none".to_owned(), |c| c.to_string()),
            run.stderr_summary()
        ))),
    }
}

/// What the classification reads of the two reports and the checkout, owned so a [`Classifier`] can borrow it.
struct Facts {
    not_judged: BTreeMap<String, NotJudgedScope>,
    skif_root_packages: BTreeSet<String>,
    foreign_paths: BTreeSet<String>,
    /// The configuration files knip could not load (`knip_config_errors`).
    knip_errors: BTreeSet<String>,
    knip_failed: BTreeSet<String>,
    knip_manifests: BTreeSet<String>,
    knip_unused_imports: BTreeSet<String>,
    skif_unused_imports: BTreeSet<String>,
    published_reach: BTreeSet<String>,
    knip_unused_credits: BTreeSet<String>,
    skif_unused_credits: BTreeSet<String>,
    bazel_credits: BTreeSet<String>,
}

impl Facts {
    fn read(skif_doc: &Value, knip_stderr: &str, skif: &Keys, knip: &Keys, root: &Path, layout: &Layout<'_>) -> Self {
        let knip_errors = knip_config_errors(knip_stderr, root);
        let knip_failed = knip_errors
            .iter()
            .map(|file| owner_dir(file, &|dir: &str| layout.is_project(dir)))
            .collect();
        let read_source = |file: &str| fs::read_to_string(root.join(file)).ok();
        let workspace_of = |file: &str| owner_dir(file, &|dir: &str| layout.is_workspace(dir));
        let (knip_unused, skif_unused) = (unused_files(knip), unused_files(skif));
        Self {
            not_judged: not_judged_dirs(skif_doc),
            skif_root_packages: skif_root_packages(skif_doc),
            foreign_paths: foreign_tsconfig_paths_keys(skif_doc, root),
            knip_errors,
            knip_failed,
            knip_manifests: knip_manifests(knip),
            knip_unused_imports: unused_file_imports(knip_unused, skif_unused, &read_source, &workspace_of),
            skif_unused_imports: unused_file_imports(skif_unused, knip_unused, &read_source, &workspace_of),
            published_reach: published_reach(knip_unused, skif_unused, layout, &read_source),
            knip_unused_credits: unused_file_credits(knip_unused, skif_unused, layout.is_file, &read_source),
            skif_unused_credits: unused_file_credits(skif_unused, knip_unused, layout.is_file, &read_source),
            bazel_credits: bazel_credits(&read_build_files(root), layout.is_file, &read_source),
        }
    }

    /// The classifier of one repository's one-sided findings (`skif` and `knip` are the findings of each side).
    fn classifier<'a>(
        &'a self,
        repo: &'a Repo,
        options: &Options,
        (skif, knip): (&'a Keys, &'a Keys),
        rush_root: bool,
        layout: &'a Layout<'a>,
    ) -> Classifier<'a> {
        Classifier {
            knip_unread: &repo.knip_unread,
            include_test_files: options.include_test_files,
            not_judged: &self.not_judged,
            knip_failed: &self.knip_failed,
            knip_manifests: &self.knip_manifests,
            foreign_paths: &self.foreign_paths,
            knip_unused_files: unused_files(knip),
            skif_unused_files: unused_files(skif),
            knip_unused_imports: &self.knip_unused_imports,
            skif_unused_imports: &self.skif_unused_imports,
            skif_root_packages: &self.skif_root_packages,
            published_reach: &self.published_reach,
            knip_unused_credits: &self.knip_unused_credits,
            skif_unused_credits: &self.skif_unused_credits,
            bazel_credits: &self.bazel_credits,
            rush_root,
            layout,
        }
    }
}

/// The narrowed packages of `not-judged-by-skif` with the globs of their scope, for `parity.json`.
fn narrowed_packages(not_judged: &BTreeMap<String, NotJudgedScope>) -> serde_json::Map<String, Value> {
    not_judged
        .iter()
        .filter_map(|(dir, scope)| match scope {
            NotJudgedScope::Within(globs) => Some((dir.clone(), json!(globs.patterns()))),
            NotJudgedScope::Package => None,
        })
        .collect()
}

fn parity_repo(
    options: &Options,
    skif: &Path,
    tools: &Tools,
    repo: &Repo,
    dir: &Path,
    skif_rev: &str,
) -> Result<Outcome> {
    let root = fs::canonicalize(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let scratch = options.out.join("scratch").join(&repo.name);
    let skif_run = match install_and_run_skif(options, skif, repo, &root, &scratch)? {
        Ok(run) => run,
        Err(reason) => return Ok(skipped(reason)),
    };
    let knip_run = match run_knip(options, tools, repo, &root, &scratch)? {
        Ok(run) => run,
        Err(reason) => return Ok(skipped(reason)),
    };
    let mut skif_found = skif_keys(&skif_run.doc, &root);
    let knip_found = knip_keys(&knip_run.doc, &root);
    align_export_labels(&mut skif_found, &knip_found);
    align_unlisted_unresolved(&mut skif_found, &knip_found);
    let manifest = manifest_reader(&root);
    let is_file = |file: &str| root.join(file).is_file();
    let build_dirs = build_dir_reader(&root);
    let layout = Layout::new(knip_run.members, &manifest, &is_file).with_build_dirs(&build_dirs);
    let facts = Facts::read(
        &skif_run.doc,
        &knip_run.run.stderr,
        &skif_found,
        &knip_found,
        &root,
        &layout,
    );
    let classifier = facts.classifier(repo, options, (&skif_found, &knip_found), knip_run.rush_root, &layout);
    let split = diff::split_known(&skif_found, &knip_found, &|side, category, key| {
        classifier.classify(side, category, key)
    });
    let (counts, samples) = diff::compare(&split.skif, &split.tool);
    let installed = is_installed(repo, &root);
    let detail = json!({
        "status": COMPARED,
        "installed": installed,
        "install_failure": skif_run.install_failure,
        "recursive": skif_run.recursive,
        "skif_rev": skif_rev,
        "knip_exit": knip_run.run.code,
        "knip_errors": facts.knip_errors,
        "knip_invalid": repo.knip_invalid,
        "known_deviations": split.known,
        "known_samples": split.samples,
        "not_judged_packages": facts.not_judged.keys().collect::<Vec<_>>(),
        "narrowed_packages": narrowed_packages(&facts.not_judged),
        "workspace_members": layout.members.len(),
        "skif_ms": skif_run.run.elapsed.as_millis(),
        "knip_ms": knip_run.run.elapsed.as_millis(),
        "skif_total": total(&skif_found),
        "knip_total": total(&knip_found),
        "categories": category_detail(&counts, &samples),
    });
    Ok(Outcome {
        entry: Entry::compared(counts)
            .with_known(split.known)
            .with_tool_errors(facts.knip_errors.len())
            .with_invalid(repo.knip_invalid.clone())
            .with_installed(installed)
            .with_run(skif_run.recursive, skif_rev),
        detail,
    })
}

/// The root `package.json` written into a checkout for the knip run; dropping it removes the file, so skif and the
/// syncpack diff never see it. An interrupted run leaves it behind, and `corpus::remove_scratch_root` removes it when
/// the checkout is next used.
struct ScratchRoot {
    path: PathBuf,
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            eprintln!(
                "parity: could not remove the scratch root manifest {}: {error}",
                self.path.display()
            );
        }
    }
}

/// The root manifest knip needs where the checkout has none: for `knip-root = "rush"`, a `package.json` in the
/// checkout whose `workspaces` are the `projectFolder`s of `rush.json`, there until the returned guard is dropped.
/// knip has no Rush support and needs a root manifest; running in the checkout itself gives it the `node_modules` of
/// the install. `None` when knip runs in the checkout as it is; `Err` says why the manifest could not be written.
fn knip_root(repo: &Repo, root: &Path) -> Result<Option<ScratchRoot>, String> {
    match repo.knip_root.as_deref() {
        None => Ok(None),
        Some(RUSH_KNIP_ROOT) => {
            let rush =
                fs::read_to_string(root.join("rush.json")).map_err(|error| format!("reading rush.json: {error}"))?;
            let manifest = rush_root_manifest(&rush)?;
            let path = root.join("package.json");
            if path.exists() && !is_scratch_root(&path) {
                return Err(format!(
                    "{} exists, so the repository needs no knip-root",
                    path.display()
                ));
            }
            fs::write(&path, manifest).map_err(|error| format!("writing the scratch package.json: {error}"))?;
            Ok(Some(ScratchRoot { path }))
        }
        Some(other) => Err(format!("unknown knip-root {other:?}")),
    }
}

/// The root `package.json` knip gets for a Rush repository: private, marked as the harness's own
/// ([`SCRATCH_ROOT_MARK`]), with the `projectFolder` of every project of `rush.json` (JSON with comments) as its
/// `workspaces`.
fn rush_root_manifest(rush_json: &str) -> Result<String, String> {
    let rush: Value =
        serde_json::from_str(&strip_jsonc(rush_json)).map_err(|error| format!("parsing rush.json: {error}"))?;
    let folders: Vec<&str> = rush
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|project| project.get("projectFolder").and_then(Value::as_str))
        .collect();
    if folders.is_empty() {
        return Err("rush.json lists no projectFolder".to_owned());
    }
    Ok(json!({ "//": SCRATCH_ROOT_MARK, "private": true, "workspaces": folders }).to_string() + "\n")
}

/// JSON with comments made plain JSON: `//` and `/* */` comments outside strings are dropped, and so is a comma
/// before a closing `]` or `}`.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(character) = chars.next() {
        if in_string {
            out.push(character);
            match character {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (character, chars.peek().copied()) {
            ('"', _) => {
                in_string = true;
                out.push(character);
            }
            ('/', Some('/')) => while chars.next_if(|next| *next != '\n').is_some() {},
            ('/', Some('*')) => {
                chars.next();
                let mut previous = ' ';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
                out.push(' ');
            }
            (']' | '}', _) => {
                let kept = out.trim_end().len();
                if out[..kept].ends_with(',') {
                    out.remove(kept - 1);
                }
                out.push(character);
            }
            _ => out.push(character),
        }
    }
    out
}

/// An empty knip configuration, so the repository's own settings (ignores, `files: off`, ignored workspaces) do not
/// hide findings. Returns its absolute path.
pub fn empty_knip_config(scratch: &Path) -> Result<PathBuf> {
    write_config(scratch, "knip-empty.json", &knip_config(&[]))
}

/// The root workspace's entry files: the defaults, then the corpus-level ones.
fn root_entry(entry: &[String]) -> Vec<String> {
    DEFAULT_ENTRY
        .iter()
        .map(|glob| (*glob).to_owned())
        .chain(entry.iter().cloned())
        .collect()
}

/// The knip configuration: empty, so the repository's own settings (ignores, `files: off`, ignored workspaces) do not
/// hide findings, apart from the corpus-level entry files of the root workspace.
fn knip_config(entry: &[String]) -> String {
    let config = if entry.is_empty() {
        json!({})
    } else {
        json!({ "workspaces": { ".": { "entry": root_entry(entry) } } })
    };
    config.to_string() + "\n"
}

/// The skif configuration that adds the corpus-level entry files to the root workspace. A JSON array of strings is a
/// TOML array as well.
fn skif_entry_config(entry: &[String]) -> String {
    format!("[js.workspaces.\".\"]\nentry = {}\n", json!(root_entry(entry)))
}

/// Writes a configuration file into the scratch directory and returns its absolute path.
fn write_config(scratch: &Path, name: &str, contents: &str) -> Result<PathBuf> {
    fs::create_dir_all(scratch).with_context(|| format!("creating {}", scratch.display()))?;
    let path = fs::canonicalize(scratch)?.join(name);
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn category_detail(counts: &BTreeMap<String, Counts>, samples: &BTreeMap<String, Samples>) -> Value {
    Value::Object(
        counts
            .iter()
            .map(|(category, found)| {
                (
                    category.clone(),
                    json!({
                        "skif_only": found.skif_only,
                        "knip_only": found.tool_only,
                        "both": found.both,
                        "samples": samples.get(category).map(|s| json!({ "skif_only": s.skif_only, "knip_only": s.tool_only })),
                    }),
                )
            })
            .collect(),
    )
}

fn markdown(tool: &str, entries: &BTreeMap<String, Entry>) -> String {
    let mut out = format!(
        "# skif findings parity with {tool}\n\nDependencies installed: {}.\n\n| Repository | Status | skif only | {tool} only | Both | Not judged by skif | Known deviations |\n| --- | --- | ---: | ---: | ---: | ---: | ---: |\n",
        installed_summary(entries)
    );
    for (name, entry) in entries {
        if entry.status == COMPARED {
            let sum = |f: fn(&Counts) -> usize| entry.categories.values().map(f).sum::<usize>();
            let unjudged = entry
                .known_deviations
                .get(NOT_JUDGED)
                .map_or(0, |c| c.skif_only + c.tool_only);
            let mut status = if entry.tool_errors == 0 {
                "compared".to_owned()
            } else {
                format!("compared; {tool} could not load {} config files", entry.tool_errors)
            };
            if let Some(reason) = &entry.invalid {
                let _ = write!(status, "; not a valid comparison: {reason}");
            }
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {unjudged} | {} |",
                table_cell(name),
                table_cell(&status),
                sum(|c| c.skif_only),
                sum(|c| c.tool_only),
                sum(|c| c.both),
                diff::known_total(&entry.known_deviations) - unjudged
            );
        } else {
            let _ = writeln!(
                out,
                "| {} | skipped: {} | - | - | - | - | - |",
                table_cell(name),
                table_cell(entry.reason.as_deref().unwrap_or(""))
            );
        }
    }
    out + &diff::known_section(tool, &KNIP_DEVIATIONS, entries)
}

/// Runs the comparison on the parity suite and applies the baseline gate.
///
/// # Errors
/// Fails when npx or skif is missing, the corpus cannot be fetched, a command cannot start, the report cannot be
/// written, or the gate finds drift.
pub fn run(options: &Options) -> Result<()> {
    if (options.repo_knip_config || options.include_test_files) && (options.gate.write || options.gate.check) {
        bail!(
            "--repo-knip-config and --include-test-files change what is compared: the baselines are recorded without them"
        );
    }
    if find_on_path("npx").is_none() {
        bail!("npx is required to run the pinned knip: install Node.js");
    }
    let skif = fs::canonicalize(&options.skif).with_context(|| {
        format!(
            "skif binary not found at {} (build it or pass --skif)",
            options.skif.display()
        )
    })?;
    let suite = corpus::fetch_suite(&options.corpus_file, "parity", &options.only, &options.corpus_dir)?;
    let tools = &suite.tools;
    let tool = format!("knip@{}", tools.knip);
    let skif_rev = provenance::source_rev();
    let mut entries = BTreeMap::new();
    let mut details = serde_json::Map::new();
    for (repo, dir) in &suite.repos {
        eprintln!("parity: {}", repo.name);
        let outcome = parity_repo(options, &skif, tools, repo, dir, &skif_rev)?;
        if let Some(reason) = &outcome.entry.reason {
            eprintln!("parity: {} skipped: {reason}", repo.name);
        }
        details.insert(repo.name.clone(), outcome.detail);
        entries.insert(repo.name.clone(), outcome.entry);
    }
    let table = markdown(&tool, &entries);
    fs::create_dir_all(&options.out).with_context(|| format!("creating {}", options.out.display()))?;
    let full = json!({
        "tool": tool,
        "installed": installed_summary(&entries),
        "install_requested": options.install,
        "skif_rev": skif_rev,
        "recursive": options.recursive,
        "repo_knip_config": options.repo_knip_config,
        "include_test_files": options.include_test_files,
        "repos": details,
    });
    fs::write(
        options.out.join("parity.json"),
        serde_json::to_string_pretty(&full)? + "\n",
    )?;
    fs::write(options.out.join("parity.md"), &table)?;
    println!("{table}");
    let current = Baseline {
        schema: BASELINE_SCHEMA,
        tool,
        installed: entries.values().all(|entry| entry.installed != Some(false)),
        category_set: Some(category_set()),
        entries,
    };
    diff::conclude(&options.gate, &current)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn set(keys: &Keys, category: &str) -> Vec<String> {
        keys[category].iter().cloned().collect()
    }

    #[test]
    fn normalizes_absolute_and_relative_paths() {
        let root = Path::new("/work/repo");
        assert_eq!(normalize("/work/repo/a/b.ts", root), "a/b.ts");
        assert_eq!(normalize("./a/b.ts", root), "a/b.ts");
        assert_eq!(normalize("a\\b.ts", root), "a/b.ts");
        assert_eq!(normalize("/work/repository/a.ts", root), "/work/repository/a.ts");
    }

    #[test]
    fn maps_skif_diagnostics_to_knip_categories() {
        let doc = json!({ "diagnostics": [
            { "rule": "unused-dependencies", "message": "`left-pad` is never used", "location": { "path": "packages/a/package.json" } },
            { "rule": "unlisted-dependencies", "message": "`react` is imported", "dependency": "react", "location": { "path": "/r/src/a.ts" } },
            { "rule": "unused-files", "message": "`src/dead.ts` is not reachable", "location": { "path": "src/dead.ts" } },
            { "rule": "duplicate-exports", "message": "`local` is exported as `b` and `a`", "location": { "path": "src/d.ts" } },
            { "rule": "version-mismatch", "message": "x", "location": { "path": "package.json" } },
        ]});
        let keys = skif_keys(&doc, Path::new("/r"));
        assert_eq!(set(&keys, "dependencies"), ["packages/a/package.json|left-pad"]);
        assert_eq!(set(&keys, "unlisted"), ["src/a.ts|react"]);
        assert_eq!(set(&keys, "files"), ["src/dead.ts"]);
        assert_eq!(set(&keys, "duplicates"), ["src/d.ts|a|b"]);
        assert_eq!(keys.len(), 4);
    }

    #[test]
    fn maps_knip_issues_of_every_shape() {
        let doc = json!({ "files": ["old.ts"], "issues": [
            { "file": "package.json", "dependencies": [{ "name": "zzz" }], "devDependencies": [] },
            { "file": "src/dead.ts", "files": [{ "name": "src/dead.ts" }] },
            { "file": "src/e.ts", "enumMembers": { "Color": [{ "name": "Red" }] }, "exports": [{ "name": "x", "line": 1 }],
              "duplicates": [[{ "name": "b" }, { "name": "a" }]], "nsTypes": [{ "name": "T" }], "classMembers": [{ "name": "m" }] },
        ]});
        let keys = knip_keys(&doc, Path::new("/r"));
        assert_eq!(set(&keys, "dependencies"), ["package.json|zzz"]);
        assert_eq!(set(&keys, "files"), ["old.ts", "src/dead.ts"]);
        assert_eq!(set(&keys, "enumMembers"), ["src/e.ts|Red"]);
        assert_eq!(set(&keys, "exports"), ["src/e.ts|x"]);
        assert_eq!(set(&keys, "duplicates"), ["src/e.ts|a|b"]);
        assert_eq!(set(&keys, "nsTypes"), ["src/e.ts|T"]);
        assert!(!keys.contains_key("namespaceMembers"));
        assert!(!keys.contains_key("devDependencies"));
        assert!(!keys.contains_key("classMembers"));
    }

    #[test]
    fn equal_findings_agree_completely() {
        let skif = json!({ "diagnostics": [
            { "rule": "unused-exports", "message": "export `x` is never imported", "location": { "path": "src/e.ts" } },
        ]});
        let knip = json!({ "issues": [{ "file": "src/e.ts", "exports": [{ "name": "x" }] }] });
        let (counts, _) = diff::compare(&skif_keys(&skif, Path::new("/r")), &knip_keys(&knip, Path::new("/r")));
        assert_eq!(
            counts["exports"],
            Counts {
                skif_only: 0,
                tool_only: 0,
                both: 1
            }
        );
    }

    #[test]
    fn install_commands_follow_the_package_manager() {
        let repo = |pm: &str, install: bool| Repo {
            name: "r".into(),
            url: String::new(),
            rev: "0".repeat(40),
            package_manager: pm.into(),
            suites: vec!["parity".into()],
            sparse: Vec::new(),
            install,
            notes: "why".into(),
            recursive: false,
            knip_unread: Vec::new(),
            install_timeout_secs: None,
            install_command: Vec::new(),
            entry: Vec::new(),
            bench_knip_args: Vec::new(),
            bench_knip_failure: None,
            knip_invalid: None,
            knip_root: None,
            build_command: Vec::new(),
            build_timeout_secs: None,
        };
        let dir = Path::new("/nonexistent");
        assert_eq!(install_command(&repo("npm", true), dir).unwrap().1[0], "ci");
        assert!(
            install_command(&repo("pnpm", true), dir)
                .unwrap()
                .1
                .iter()
                .any(|a| a == "--ignore-scripts")
        );
        let mut overridden = repo("pnpm", true);
        overridden.install_command = vec!["npx".into(), "pnpm@9".into(), "install".into()];
        assert_eq!(
            install_command(&overridden, dir).unwrap(),
            ("npx".to_owned(), vec!["pnpm@9".to_owned(), "install".to_owned()])
        );
        assert!(install_command(&repo("rush", true), dir).is_err());
        assert!(install_command(&repo("pnpm", false), dir).unwrap_err().contains("why"));
    }

    #[test]
    fn member_keys_drop_the_qualifier_on_both_sides() {
        let skif = json!({ "diagnostics": [
            { "rule": "unused-enum-members", "message": "enum member `Color.Red` is never used", "location": { "path": "src/e.ts" } },
            { "rule": "unused-namespace-members", "message": "namespace member `ns.Helper` is never used", "location": { "path": "src/n.ts" } },
        ]});
        let knip = json!({ "issues": [
            { "file": "src/e.ts", "enumMembers": { "Color": [{ "name": "Red" }] } },
            { "file": "src/n.ts", "namespaceMembers": [{ "name": "ns.Helper" }] },
        ]});
        let root = Path::new("/r");
        let (counts, _) = diff::compare(&skif_keys(&skif, root), &knip_keys(&knip, root));
        assert_eq!(counts["enumMembers"].both, 1);
        assert_eq!(counts["namespaceMembers"].both, 1);
        assert_eq!(counts["enumMembers"].skif_only + counts["enumMembers"].tool_only, 0);
    }

    /// The JSON that `knip --reporter json` prints: every reported issue type is present on every row, empty or not,
    /// members are a flat list of `{ namespace, name }` (the parent first, the bare member name second), and a module
    /// that is only read through a namespace import lists its unused exports and types with that namespace.
    fn knip_report() -> Value {
        json!({ "issues": [
            { "file": "src/e.ts", "binaries": [], "dependencies": [], "duplicates": [],
              "enumMembers": [{ "namespace": "Color", "name": "Green", "line": 1, "col": 26, "pos": 25 }],
              "exports": [], "files": [], "namespaceMembers": [],
              "types": [{ "name": "Unused", "line": 2, "col": 13, "pos": 45 }], "unlisted": [], "unresolved": [] },
            { "file": "src/n.ts", "enumMembers": [],
              "exports": [{ "namespace": "ns", "name": "other", "line": 2, "col": 14, "pos": 36 }],
              "namespaceMembers": [{ "namespace": "Helpers", "name": "run", "line": 4, "col": 3, "pos": 80 }],
              "types": [{ "namespace": "ns", "name": "B", "line": 3, "col": 13, "pos": 60 }] },
        ]})
    }

    #[test]
    fn maps_the_json_knip_prints() {
        let keys = knip_keys(&knip_report(), Path::new("/r"));
        assert_eq!(set(&keys, "enumMembers"), ["src/e.ts|Green"]);
        assert_eq!(set(&keys, "types"), ["src/e.ts|Unused"]);
        assert_eq!(set(&keys, "nsExports"), ["src/n.ts|other"]);
        assert_eq!(set(&keys, "nsTypes"), ["src/n.ts|B"]);
        assert_eq!(set(&keys, "namespaceMembers"), ["src/n.ts|run"]);
        assert!(!keys.contains_key("exports"));
        assert!(!keys.contains_key("duplicates"));
    }

    #[test]
    fn every_member_and_namespace_finding_agrees_with_the_json_knip_prints() {
        let skif = json!({ "diagnostics": [
            { "rule": "unused-enum-members", "message": "enum member `Color.Green` is never used", "location": { "path": "src/e.ts" } },
            { "rule": "unused-types", "message": "type `Unused` is never imported by another module", "location": { "path": "src/e.ts" } },
            { "rule": "unused-namespace-exports", "message": "export `other` is never read through a namespace import", "location": { "path": "src/n.ts" } },
            { "rule": "unused-namespace-types", "message": "type `B` is never read through a namespace import", "location": { "path": "src/n.ts" } },
            { "rule": "unused-namespace-members", "message": "namespace member `Helpers.run` is never used", "location": { "path": "src/n.ts" } },
        ]});
        let root = Path::new("/r");
        let (counts, _) = diff::compare(&skif_keys(&skif, root), &knip_keys(&knip_report(), root));
        let both = Counts {
            skif_only: 0,
            tool_only: 0,
            both: 1,
        };
        for category in ["enumMembers", "types", "nsExports", "nsTypes", "namespaceMembers"] {
            assert_eq!(counts[category], both, "{category}");
        }
    }

    #[test]
    fn corpus_entry_files_reach_both_tools_after_the_defaults() {
        assert_eq!(knip_config(&[]), "{}\n");
        let entry = ["src/vs/workbench/workbench.desktop.main.ts".to_owned()];
        let knip: Value = serde_json::from_str(&knip_config(&entry)).unwrap();
        let expected = json!([DEFAULT_ENTRY[0], DEFAULT_ENTRY[1], entry[0]]);
        assert_eq!(knip["workspaces"]["."]["entry"], expected);
        let skif: toml::Value = toml::from_str(&skif_entry_config(&entry)).unwrap();
        let listed: Vec<&str> = skif["js"]["workspaces"]["."]["entry"]
            .as_array()
            .unwrap()
            .iter()
            .map(|glob| glob.as_str().unwrap())
            .collect();
        assert_eq!(listed, [DEFAULT_ENTRY[0], DEFAULT_ENTRY[1], entry[0].as_str()]);
    }

    #[test]
    fn finds_the_report_among_banners() {
        let noisy = b"Platform libraries usePool true\n[dotenv@17] injected env (0) from .env\n{\"issues\":[{\"file\":\"a.ts\",\"exports\":[{\"name\":\"x\"}]}]}\n";
        let doc = parse_knip_report(noisy).unwrap();
        assert_eq!(doc["issues"][0]["file"], "a.ts");
        let skipped_object = b"{\"log\":1}\n{\"files\":[\"b.ts\"],\"issues\":[]}\n";
        assert_eq!(parse_knip_report(skipped_object).unwrap()["files"][0], "b.ts");
        let pretty = b"banner\n{\n  \"issues\": []\n}\n";
        assert!(parse_knip_report(pretty).is_ok());
    }

    #[test]
    fn reports_why_no_report_was_found() {
        assert_eq!(parse_knip_report(b"  \n").unwrap_err(), "stdout is empty");
        assert!(
            parse_knip_report(b"just text\n")
                .unwrap_err()
                .contains("no knip report")
        );
        assert!(
            parse_knip_report(b"{\"issues\": [oops\n")
                .unwrap_err()
                .contains("first parse error")
        );
        assert!(parse_knip_report(b"{\"other\":1}\n").is_err());
    }

    #[test]
    fn classifies_unused_files_knip_alone_reports() {
        assert_eq!(classify_file("src/app.css", false), Some(NON_CODE_FILE));
        assert_eq!(classify_file("docs/index.mdx", true), Some(NON_CODE_FILE));
        assert_eq!(classify_file("pkg/test/a.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/src/a.spec.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/src/a_spec.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/__fixtures__/a.ts", false), Some(FIXTURE_FILE));
        assert_eq!(classify_file("pkg/fixtures/test/a.ts", false), Some(FIXTURE_FILE));
        assert_eq!(
            classify_file("pkg/x/colorize-fixtures/a.ts", false),
            None,
            "skif judges other test-data names"
        );
        assert_eq!(classify_file("pkg/testdata/a.ts", false), None);
        assert_eq!(classify_file("pkg/fixture/test/a.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/test/a.ts", true), None);
        assert_eq!(classify_file("pkg/fixtures/a.ts", true), None);
        assert_eq!(classify_file("pkg/src/a.ts", false), None);
        assert_eq!(classify_file("pkg/src/latest.tsx", false), None);
        assert_eq!(classify_file("typings/legacy.d.ts", false), Some(DECLARATION_FILE));
        assert_eq!(
            classify_file("pkg/test/globals.d.ts", true),
            Some(DECLARATION_FILE),
            "whatever `--include-test-files` says"
        );
        assert_eq!(classify_file("lib/watch.d.mts", false), None, "skif judges `.d.mts`");
        assert_eq!(classify_file("lib/types.d.cts", false), None);
        assert_eq!(classify_file("src/d.ts", false), None);
    }

    #[test]
    fn unlisted_imports_only_foreign_tsconfig_paths_map_split_off() {
        let skif_doc = json!({ "diagnostics": [
            { "rule": "unlisted-dependencies", "message": "`shared/value` resolves only through the `paths` of `packages/app/tsconfig.json`; add the mapping to this package's tsconfig or import it by package name",
              "location": { "path": "/r/packages/lib/src/index.ts" }, "dependency": "shared", "detail": "ForeignTsconfigPaths" },
            { "rule": "unlisted-dependencies", "message": "`left-pad` is imported but not listed in `package.json`",
              "location": { "path": "/r/packages/lib/src/index.ts" }, "dependency": "left-pad" },
            { "rule": "unused-files", "message": "`x.ts` is unused", "location": { "path": "/r/packages/lib/src/x.ts" },
              "detail": "ForeignTsconfigPaths" },
        ]});
        let root = Path::new("/r");
        let foreign = foreign_tsconfig_paths_keys(&skif_doc, root);
        assert_eq!(
            foreign.iter().map(String::as_str).collect::<Vec<_>>(),
            ["packages/lib/src/index.ts|shared"]
        );
        let skif = skif_keys(&skif_doc, root);
        let knip = Keys::from([(
            "unlisted".to_owned(),
            ["packages/lib/src/index.ts|shared"].map(str::to_owned).into(),
        )]);
        let split = diff::split_known(&skif, &Keys::new(), &|side, category, key| {
            (side == Side::Skif)
                .then(|| skif_only_deviation(category, key, &foreign))
                .flatten()
        });
        let (counts, _) = diff::compare(&split.skif, &split.tool);
        assert_eq!(
            counts["unlisted"],
            Counts {
                skif_only: 1,
                tool_only: 0,
                both: 0
            },
            "the plain unlisted import stays a difference"
        );
        assert_eq!(split.known[FOREIGN_TSCONFIG_PATHS].skif_only, 1);
        assert_eq!(split.known.len(), 1, "{:?}", split.known);
        // A finding both tools report is agreement, never a deviation.
        let both = diff::split_known(&skif, &knip, &|side, category, key| {
            (side == Side::Skif)
                .then(|| skif_only_deviation(category, key, &foreign))
                .flatten()
        });
        assert!(both.known.is_empty(), "{:?}", both.known);
        assert_eq!(
            skif_only_deviation("files", "packages/lib/src/index.ts|shared", &foreign),
            None
        );
    }

    #[test]
    fn unused_files_only_known_deviations_split_off() {
        let skif = Keys::from([("files".to_owned(), ["src/dead.ts".to_owned()].into())]);
        let knip = Keys::from([(
            "files".to_owned(),
            ["src/dead.ts", "src/a.css", "test/a.ts", "src/real.ts"]
                .map(str::to_owned)
                .into(),
        )]);
        let split = diff::split_known(&skif, &knip, &|side, category, key| {
            (side == Side::Tool && category == "files")
                .then(|| classify_file(key, false))
                .flatten()
        });
        let (counts, _) = diff::compare(&split.skif, &split.tool);
        assert_eq!(
            counts["files"],
            Counts {
                skif_only: 0,
                tool_only: 1,
                both: 1
            }
        );
        assert_eq!(split.known[NON_CODE_FILE].tool_only, 1);
        assert_eq!(split.known[TEST_FILE].tool_only, 1);
    }

    #[test]
    fn the_same_export_under_another_label_is_agreement() {
        let mut skif = Keys::from([
            (
                "exports".to_owned(),
                ["a.ts|ns", "a.ts|plain", "a.ts|mine"].map(str::to_owned).into(),
            ),
            ("types".to_owned(), ["b.ts|Shared"].map(str::to_owned).into()),
        ]);
        let knip = Keys::from([
            ("exports".to_owned(), ["a.ts|plain"].map(str::to_owned).into()),
            ("nsExports".to_owned(), ["a.ts|ns"].map(str::to_owned).into()),
            ("types".to_owned(), ["b.ts|Shared"].map(str::to_owned).into()),
        ]);
        align_export_labels(&mut skif, &knip);
        let (counts, _) = diff::compare(&skif, &knip);
        assert_eq!(
            counts["exports"],
            Counts {
                skif_only: 1,
                tool_only: 0,
                both: 1
            }
        );
        assert_eq!(
            counts["nsExports"],
            Counts {
                skif_only: 0,
                tool_only: 0,
                both: 1
            }
        );
        assert_eq!(counts["types"].both, 1);
    }

    #[test]
    fn an_import_one_tool_reports_unlisted_and_the_other_unresolved_is_agreement() {
        let mut skif = Keys::from([
            (
                "unlisted".to_owned(),
                strings(&[
                    "packages/a/tsconfig.json|tslib",
                    "packages/b/tsconfig.json|vite",
                    "packages/c/src/a.ts|left-pad",
                    "packages/d/tsconfig.json|vitest",
                    "packages/f/src/a.ts|shared",
                ]),
            ),
            (
                "unresolved".to_owned(),
                strings(&["packages/e/src/a.ts|@scope/x/sub", "packages/e/src/b.ts|./gone"]),
            ),
        ]);
        let knip = Keys::from([
            (
                "unresolved".to_owned(),
                strings(&[
                    "packages/a/tsconfig.json|tslib",
                    "packages/b/tsconfig.json|vite/client",
                    "packages/c/src/b.ts|left-pad",
                    "packages/d/tsconfig.json|vitest/globals",
                ]),
            ),
            (
                "unlisted".to_owned(),
                strings(&[
                    "packages/d/tsconfig.json|vitest",
                    "packages/e/src/a.ts|@scope/x",
                    "packages/e/src/b.ts|gone",
                ]),
            ),
        ]);
        align_unlisted_unresolved(&mut skif, &knip);
        assert_eq!(
            set(&skif, "unresolved"),
            [
                "packages/a/tsconfig.json|tslib",
                "packages/b/tsconfig.json|vite/client",
                "packages/e/src/b.ts|./gone",
            ]
        );
        assert_eq!(
            set(&skif, "unlisted"),
            [
                "packages/c/src/a.ts|left-pad",
                "packages/d/tsconfig.json|vitest",
                "packages/e/src/a.ts|@scope/x",
                "packages/f/src/a.ts|shared",
            ],
            "another file, a finding knip shares already and a relative specifier stay where they are"
        );
        let (counts, _) = diff::compare(&skif, &knip);
        assert_eq!(counts["unresolved"].both, 2);
        assert_eq!(counts["unlisted"].both, 2);
    }

    #[test]
    fn knip_only_findings_in_packages_skif_did_not_judge_split_off() {
        let skif_doc = json!({ "diagnostics": [], "incomplete-packages": [
            { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-spread" }] },
        ]});
        let dirs = not_judged_dirs(&skif_doc);
        assert_eq!(
            dirs,
            BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)])
        );
        assert_eq!(
            not_judged_dirs(&json!({ "incomplete-packages": [{ "dir": ".", "scope": "package" }] })),
            BTreeMap::from([(String::new(), NotJudgedScope::Package)])
        );
        assert!(not_judged_dirs(&json!({ "diagnostics": [] })).is_empty());

        let packages = ["packages/web", "packages/web/nested", "packages/api"];
        let is_package = |dir: &str| packages.contains(&dir);
        assert_eq!(owner_dir("packages/web/src/a.ts", &is_package), "packages/web");
        assert_eq!(owner_dir("packages/web/package.json|vite", &is_package), "packages/web");
        assert_eq!(
            owner_dir("packages/web/nested/a.ts", &is_package),
            "packages/web/nested"
        );
        assert_eq!(owner_dir("scripts/a.ts", &is_package), "");

        let knip = Keys::from([
            (
                "files".to_owned(),
                [
                    "packages/web/src/orphan.ts",
                    "packages/web/nested/orphan.ts",
                    "packages/api/orphan.ts",
                ]
                .map(str::to_owned)
                .into(),
            ),
            (
                "devDependencies".to_owned(),
                ["packages/web/package.json|vite"].map(str::to_owned).into(),
            ),
            (
                "exports".to_owned(),
                ["packages/web/src/a.ts|unused"].map(str::to_owned).into(),
            ),
        ]);
        let split = diff::split_known(&Keys::new(), &knip, &|side, category, key| {
            (side == Side::Tool && not_judged(category, key, &dirs, &is_package)).then_some(NOT_JUDGED)
        });
        assert_eq!(
            split.known[NOT_JUDGED],
            diff::KnownCounts {
                skif_only: 0,
                tool_only: 3
            }
        );
        let (counts, _) = diff::compare(&split.skif, &split.tool);
        assert_eq!(
            counts["files"].tool_only, 2,
            "a nested complete package and another package stay one-sided"
        );
        assert!(
            !counts.contains_key("exports"),
            "skif does not judge the exports of an incomplete package (ADR 0068)"
        );
        assert!(!counts.contains_key("devDependencies"));
    }

    #[test]
    fn not_judged_covers_exports_types_and_members_but_not_unlisted_or_duplicates() {
        let dirs = BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)]);
        let is_package = |dir: &str| dir == "packages/web";
        for category in [
            "exports",
            "types",
            "nsExports",
            "nsTypes",
            "enumMembers",
            "namespaceMembers",
        ] {
            assert!(
                not_judged(category, "packages/web/src/a.ts|x", &dirs, &is_package),
                "{category}"
            );
        }
        for category in [
            "unlisted",
            "unresolved",
            "binaries",
            "duplicates",
            "optionalPeerDependencies",
        ] {
            assert!(
                !not_judged(category, "packages/web/src/a.ts|x", &dirs, &is_package),
                "{category}"
            );
        }
        assert!(!not_judged("exports", "packages/api/src/a.ts|x", &dirs, &is_package));
    }

    #[test]
    fn a_narrowed_scope_sets_apart_only_the_files_its_globs_match_and_every_dependency() {
        let skif_doc = json!({ "diagnostics": [], "incomplete-packages": [
            { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-config", "plugin": "storybook" }],
              "scope": { "within": ["src/**/*.stories.tsx"] } },
            { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-config", "plugin": "vitest" }],
              "scope": { "within": ["test/*.ts", "src/**/*.stories.tsx"] } },
            { "name": "root", "dir": ".", "reasons": [{ "cause": "unknown-config" }],
              "scope": { "within": ["scripts/*.ts"] } },
            { "name": "api", "dir": "packages/api", "reasons": [{ "cause": "unknown-spread" }], "scope": "package" },
        ]});
        let dirs = not_judged_dirs(&skif_doc);
        assert_eq!(
            dirs,
            BTreeMap::from([
                (String::new(), NotJudgedScope::within(vec!["scripts/*.ts".to_owned()])),
                ("packages/api".to_owned(), NotJudgedScope::Package),
                (
                    "packages/web".to_owned(),
                    NotJudgedScope::within(vec!["src/**/*.stories.tsx".to_owned(), "test/*.ts".to_owned()])
                ),
            ]),
            "the scopes of one directory are joined"
        );
        let packages = ["packages/web", "packages/api"];
        let is_package = |dir: &str| packages.contains(&dir);
        for (category, key) in [
            ("files", "packages/web/src/ui/Button.stories.tsx"),
            ("exports", "packages/web/src/ui/Button.stories.tsx|Primary"),
            ("types", "packages/web/test/setup.ts|Fixture"),
            ("enumMembers", "packages/web/test/setup.ts|Red"),
            ("dependencies", "packages/web/package.json|react"),
            ("devDependencies", "packages/web/package.json|vitest"),
            ("files", "scripts/release.ts"),
            ("devDependencies", "package.json|tsx"),
            ("files", "packages/api/src/orphan.ts"),
        ] {
            assert!(not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
        for (category, key) in [
            ("files", "packages/web/src/ui/Button.tsx"),
            ("exports", "packages/web/src/ui/Button.tsx|Button"),
            ("files", "packages/web/test/deep/setup.ts"),
            ("files", "scripts/deep/release.ts"),
            ("files", "tools/a.ts"),
            ("unlisted", "packages/web/src/ui/Button.stories.tsx|react"),
        ] {
            assert!(!not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
        assert_eq!(
            not_judged_dirs(&json!({ "incomplete-packages": [
                { "dir": "packages/web", "scope": { "within": ["src/*.ts"] } },
                { "dir": "packages/web", "scope": "package" },
            ]})),
            BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)]),
            "a whole-package entry wins over a narrowed one"
        );
    }

    #[test]
    fn a_narrowed_scope_reaches_the_files_of_the_packages_below_its_package() {
        let dirs = BTreeMap::from([
            (String::new(), NotJudgedScope::within(vec!["packages/*/e2e/**".to_owned()])),
            (
                "packages/core".to_owned(),
                NotJudgedScope::within(vec!["**/__tests__/**".to_owned()]),
            ),
            ("apps".to_owned(), NotJudgedScope::Package),
        ]);
        let packages = [
            "packages/core",
            "packages/core/nested",
            "packages/web",
            "apps",
            "apps/site",
        ];
        let is_package = |dir: &str| packages.contains(&dir);
        for (category, key) in [
            ("files", "packages/core/nested/src/__tests__/helper.ts"),
            ("exports", "packages/core/nested/src/__tests__/helper.ts|setup"),
            ("types", "packages/web/e2e/run.ts|Options"),
            ("files", "packages/web/e2e/fixtures/app.ts"),
        ] {
            assert!(not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
        for (category, key) in [
            ("files", "packages/core/nested/src/helper.ts"),
            ("files", "packages/web/src/e2e/run.ts"),
            ("devDependencies", "packages/web/package.json|playwright"),
            ("devDependencies", "packages/core/nested/package.json|vitest"),
            ("unlisted", "packages/web/e2e/run.ts|react"),
            ("files", "apps/site/src/orphan.ts"),
        ] {
            assert!(!not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
    }

    #[test]
    fn reads_the_config_files_knip_could_not_load() {
        let stderr = "❌ Invalid environment variables: [\n\
            ERROR: Error loading /r/apps/expo/metro.config.js (Cannot find module 'metro-cache')\n\
            ERROR: Please fix or visit https://knip.dev/reference/known-issues\n\
            ERROR: Error loading packages/db/drizzle.config.ts (Missing POSTGRES_URL)\n";
        let errors = knip_config_errors(stderr, Path::new("/r"));
        assert_eq!(
            errors,
            BTreeSet::from([
                "apps/expo/metro.config.js".to_owned(),
                "packages/db/drizzle.config.ts".to_owned()
            ])
        );
        assert!(knip_config_errors("ERROR: something else\n", Path::new("/r")).is_empty());
    }

    #[test]
    fn the_summary_counts_not_judged_findings_apart_from_the_known_deviations() {
        let counts = BTreeMap::from([(
            "files".to_owned(),
            Counts {
                skif_only: 1,
                tool_only: 2,
                both: 3,
            },
        )]);
        let known = diff::Known::from([
            (
                NOT_JUDGED.to_owned(),
                diff::KnownCounts {
                    skif_only: 0,
                    tool_only: 4,
                },
            ),
            (
                NON_CODE_FILE.to_owned(),
                diff::KnownCounts {
                    skif_only: 0,
                    tool_only: 5,
                },
            ),
        ]);
        let entries = BTreeMap::from([
            ("r".to_owned(), Entry::compared(counts.clone()).with_known(known)),
            ("s".to_owned(), Entry::compared(counts.clone()).with_tool_errors(2)),
            (
                "t".to_owned(),
                Entry::compared(counts)
                    .with_tool_errors(1)
                    .with_invalid(Some("configs import unbuilt packages".to_owned())),
            ),
            (
                "u".to_owned(),
                Entry::skipped("knip failed (exit 2): no report; stderr: a | b\nc"),
            ),
        ]);
        let table = markdown("knip", &entries);
        assert!(table.contains("Dependencies installed: no."), "{table}");
        assert!(
            table.contains(
                "| t | compared; knip could not load 1 config files; not a valid comparison: configs import unbuilt packages | 1 | 2 | 3 | 0 | 0 |"
            ),
            "{table}"
        );
        assert!(table.contains("| r | compared | 1 | 2 | 3 | 4 | 5 |"), "{table}");
        assert!(
            table.contains("| u | skipped: knip failed (exit 2): no report; stderr: a \\| b c | - | - | - | - | - |\n"),
            "{table}"
        );
        assert!(
            table.contains("| s | compared; knip could not load 2 config files | 1 | 2 | 3 | 0 | 0 |"),
            "{table}"
        );
        assert!(table.contains("`not-judged-by-skif`"), "{table}");
    }

    #[cfg(unix)]
    #[test]
    fn a_build_runs_within_its_own_time_limit_and_a_failed_one_writes_no_marker() {
        let dir = std::env::temp_dir().join(format!("skif-bench-parity-build-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let scratch = dir.join("scratch");
        let mut repo: Repo = toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"pnpm\"\n\
             install-command = [\"sh\", \"-c\", \"exit 0\"]\n\
             build-command = [\"sleep\", \"30\"]\nbuild-timeout-secs = 1\n",
            "a".repeat(40)
        ))
        .unwrap();
        assert_eq!(repo.build_timeout_secs, Some(1));
        let reason = install(&repo, &dir, &scratch, Duration::from_secs(600)).unwrap();
        assert_eq!(reason.as_deref(), Some("sleep build timed out after 1 s"));
        assert!(
            !is_installed(&repo, &dir),
            "a timed-out build leaves the checkout not installed"
        );

        // The builds below finish at once; a generous limit keeps a loaded machine from timing them out.
        repo.build_timeout_secs = Some(120);
        repo.build_command = vec!["sh".to_owned(), "-c".to_owned(), "exit 4".to_owned()];
        let reason = install(&repo, &dir, &scratch, Duration::from_secs(600))
            .unwrap()
            .unwrap();
        assert!(reason.starts_with("sh build failed"), "{reason}");
        assert!(!is_installed(&repo, &dir));

        repo.build_command = vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()];
        assert_eq!(install(&repo, &dir, &scratch, Duration::from_secs(600)).unwrap(), None);
        assert!(is_installed(&repo, &dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_install_state_comes_from_the_checkout_markers() {
        let dir = std::env::temp_dir().join(format!("skif-bench-parity-installed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut repo: Repo = toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"pnpm\"\n",
            "a".repeat(40)
        ))
        .unwrap();
        assert!(!is_installed(&repo, &dir));
        fs::write(dir.join(INSTALLED_MARKER), format!("{}\n", "a".repeat(40))).unwrap();
        assert!(is_installed(&repo, &dir));
        repo.build_command = vec!["pnpm".to_owned(), "run".to_owned(), "build".to_owned()];
        assert!(
            !is_installed(&repo, &dir),
            "a marker written without the build is stale"
        );
        fs::write(dir.join(INSTALLED_MARKER), install_marker(&repo)).unwrap();
        assert!(is_installed(&repo, &dir));
        assert_eq!(
            install_marker(&repo),
            format!("{}\nbuild: pnpm run build", "a".repeat(40))
        );
        repo.install_command = vec!["pnpm".to_owned(), "install".to_owned()];
        assert!(
            !is_installed(&repo, &dir),
            "a marker written with another install command is stale"
        );
        assert_eq!(
            install_marker(&repo),
            format!("{}\ninstall: pnpm install\nbuild: pnpm run build", "a".repeat(40))
        );
        fs::write(dir.join(INSTALLED_MARKER), install_marker(&repo)).unwrap();
        assert!(is_installed(&repo, &dir));
        repo.rev = "b".repeat(40);
        assert!(!is_installed(&repo, &dir), "a marker of another commit is stale");
        fs::remove_dir_all(&dir).unwrap();

        let entry = |installed: bool| Entry::compared(BTreeMap::new()).with_installed(installed);
        let mut entries = BTreeMap::from([
            ("a".to_owned(), entry(true)),
            ("b".to_owned(), entry(true)),
            ("skipped".to_owned(), Entry::skipped("knip failed")),
        ]);
        assert_eq!(installed_summary(&entries), "yes");
        entries.insert("c".to_owned(), entry(false));
        assert_eq!(installed_summary(&entries), "partial (2 of 3)");
        assert_eq!(
            installed_summary(&BTreeMap::from([("c".to_owned(), entry(false))])),
            "no"
        );
    }

    #[test]
    fn the_recorded_category_set_holds_every_compared_category_and_deviation() {
        let set = category_set();
        assert_eq!(set.compared.len(), KNIP_CATEGORIES.len());
        assert!(
            set.compared.windows(2).all(|pair| pair[0] < pair[1]),
            "sorted: {:?}",
            set.compared
        );
        for id in [
            IN_KNIP_UNUSED_FILE,
            IN_SKIF_UNUSED_FILE,
            USED_IN_KNIP_UNUSED_FILE,
            USED_IN_SKIF_UNUSED_FILE,
            IMPORTED_BY_KNIP_UNUSED_FILE,
            IMPORTED_BY_SKIF_UNUSED_FILE,
            ENTRY_MODULE_DUPLICATE,
            BAZEL_BUILD_FILE,
            DECLARATION_FILE,
            KNIP_CONFIG_ERROR,
            NOT_JUDGED,
        ] {
            assert!(set.known_deviations.iter().any(|known| known == id), "{id}");
        }
        assert_eq!(set.known_deviations.len(), KNIP_DEVIATIONS.len());
    }

    #[test]
    fn a_rush_repository_gets_a_root_manifest_from_rush_json() {
        let rush = r#"/**
 * Rush's main config, see https://rushjs.io // not a comment end
 */
{
  "$schema": "https://developer.microsoft.com/json-schemas/rush/v5/rush.schema.json",
  // "projects": [] in a comment is ignored
  "rushVersion": "5.0.0",
  "note": "a \"quoted\" /* string */ stays",
  "projects": [
    { "packageName": "@rushstack/heft", "projectFolder": "apps/heft" },
    /* a block comment */
    { "packageName": "@rushstack/node-core-library", "projectFolder": "libraries/node-core-library", },
  ],
}
"#;
        let plain: Value = serde_json::from_str(&strip_jsonc(rush)).unwrap();
        assert_eq!(plain["note"], r#"a "quoted" /* string */ stays"#);
        assert_eq!(plain["projects"].as_array().unwrap().len(), 2);
        let manifest: Value = serde_json::from_str(&rush_root_manifest(rush).unwrap()).unwrap();
        assert_eq!(
            manifest,
            json!({ "//": SCRATCH_ROOT_MARK, "private": true, "workspaces": ["apps/heft", "libraries/node-core-library"] })
        );
        assert!(
            rush_root_manifest(r#"{ "projects": [] }"#)
                .unwrap_err()
                .contains("no projectFolder")
        );
        assert!(
            rush_root_manifest("{ not json")
                .unwrap_err()
                .contains("parsing rush.json")
        );
    }

    #[test]
    fn knip_gets_a_root_manifest_in_a_rush_checkout_only_while_it_runs() {
        let mut repo: Repo = toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"rush\"\n",
            "a".repeat(40)
        ))
        .unwrap();
        let missing = Path::new("/nonexistent/checkout");
        assert!(knip_root(&repo, missing).unwrap().is_none());
        repo.knip_root = Some("lerna".to_owned());
        assert!(knip_root(&repo, missing).err().unwrap().contains("unknown knip-root"));
        repo.knip_root = Some(RUSH_KNIP_ROOT.to_owned());
        assert!(knip_root(&repo, missing).err().unwrap().contains("reading rush.json"));

        let root = std::env::temp_dir().join(format!("skif-bench-parity-rush-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("rush.json"),
            r#"{ "projects": [{ "packageName": "a", "projectFolder": "apps/a" }] }"#,
        )
        .unwrap();
        let manifest = root.join("package.json");
        let guard = knip_root(&repo, &root).unwrap().unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
        assert_eq!(written["workspaces"], json!(["apps/a"]));
        assert!(is_scratch_root(&manifest));
        drop(guard);
        assert!(
            !manifest.exists(),
            "the checkout has no root manifest after the knip run"
        );

        fs::write(&manifest, written.to_string()).unwrap();
        drop(knip_root(&repo, &root).unwrap().unwrap());
        assert!(
            !manifest.exists(),
            "a manifest an interrupted run left is replaced and removed"
        );

        fs::write(&manifest, r#"{ "name": "real" }"#).unwrap();
        assert!(knip_root(&repo, &root).err().unwrap().contains("needs no knip-root"));
        assert_eq!(fs::read_to_string(&manifest).unwrap(), r#"{ "name": "real" }"#);
        fs::remove_dir_all(&root).unwrap();
    }

    fn strings(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    static NO_PATHS: [String; 0] = [];
    static NO_KEYS: BTreeSet<String> = BTreeSet::new();
    static ALL_JUDGED: BTreeMap<String, NotJudgedScope> = BTreeMap::new();

    /// A classifier over `layout` that knows nothing of the two reports: no path knip does not read, no package skif
    /// left unjudged or knip could not load a config of, no unused file and nothing one imports, no root of a
    /// recursive run and no Rush root. A test sets the facts it needs with `..bare(&layout)`.
    fn bare<'a>(layout: &'a Layout<'a>) -> Classifier<'a> {
        Classifier {
            knip_unread: &NO_PATHS,
            include_test_files: false,
            not_judged: &ALL_JUDGED,
            knip_failed: &NO_KEYS,
            knip_manifests: &NO_KEYS,
            foreign_paths: &NO_KEYS,
            knip_unused_files: &NO_KEYS,
            skif_unused_files: &NO_KEYS,
            knip_unused_imports: &NO_KEYS,
            skif_unused_imports: &NO_KEYS,
            skif_root_packages: &NO_KEYS,
            published_reach: &NO_KEYS,
            knip_unused_credits: &NO_KEYS,
            skif_unused_credits: &NO_KEYS,
            bazel_credits: &NO_KEYS,
            rush_root: false,
            layout,
        }
    }

    /// Runs `check` with a classifier for a checkout with these workspace members, manifests (by directory) and
    /// `package.json` files knip reports on, and no other facts.
    fn with_classifier(
        members: &[&str],
        manifests: &[(&str, Value)],
        judged_by_knip: &[&str],
        check: impl FnOnce(&Classifier<'_>),
    ) {
        let read: BTreeMap<String, Rc<Value>> = manifests
            .iter()
            .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
            .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(members), &manifest, &every_file);
        let judged = strings(judged_by_knip);
        check(&Classifier {
            knip_manifests: &judged,
            ..bare(&layout)
        });
    }

    #[test]
    fn fixture_findings_cover_every_category_skif_is_silent_in_there() {
        with_classifier(&[], &[], &[], |classifier| {
            for category in [
                "files",
                "exports",
                "types",
                "nsExports",
                "nsTypes",
                "enumMembers",
                "namespaceMembers",
                "unresolved",
                "unlisted",
            ] {
                let key = if category == "files" {
                    "pkg/__fixtures__/a.ts"
                } else {
                    "pkg/__fixtures__/a.ts|x"
                };
                assert_eq!(
                    classifier.classify(Side::Tool, category, key),
                    Some(FIXTURE_FILE),
                    "{category}"
                );
            }
            for category in ["duplicates", "binaries", "dependencies"] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, "pkg/fixtures/a.ts|x"),
                    None,
                    "{category}"
                );
            }
            assert_eq!(
                classifier.classify(Side::Tool, "unresolved", "pkg/fixture/a.ts|./b"),
                None
            );
            assert_eq!(classifier.classify(Side::Tool, "unresolved", "fixtures.ts|./b"), None);
            assert_eq!(
                classifier.classify(Side::Skif, "unresolved", "pkg/fixtures/a.ts|./b"),
                None
            );
        });
    }

    #[test]
    fn findings_only_skif_reports_on_manifests_knip_does_not_judge_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a" })),
            (
                "examples/app",
                json!({ "name": "app", "dependencies": { "left-pad": "1" } }),
            ),
            ("judged", json!({ "name": "judged" })),
        ];
        with_classifier(&["packages/a"], &manifests, &["judged/package.json"], |classifier| {
            for (category, key) in [
                ("dependencies", "examples/app/package.json|left-pad"),
                ("devDependencies", "examples/app/package.json|vitest"),
                ("optionalPeerDependencies", "examples/app/package.json|react"),
                ("binaries", "examples/app/package.json|turbo"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Skif, category, key),
                    Some(KNIP_UNJUDGED_MANIFEST),
                    "{key}"
                );
            }
            for (category, key) in [
                ("unlisted", "examples/app/src/a.ts|react"),
                ("unlisted", "examples/app/.eslintrc.json|@nrwl/eslint-plugin-nx"),
                ("binaries", "examples/app/.github/workflows/ci.yml|turbo"),
                ("files", "examples/app/src/orphan.ts"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Skif, category, key),
                    Some(NON_MEMBER_PROJECT),
                    "{category} {key}"
                );
            }
            for (category, key) in [
                ("dependencies", "packages/a/package.json|left-pad"),
                ("dependencies", "package.json|left-pad"),
                ("dependencies", "judged/package.json|left-pad"),
                ("binaries", "judged/package.json|turbo"),
                ("exports", "examples/app/src/a.ts|unused"),
                ("unresolved", "examples/app/src/a.ts|./gone"),
                ("unlisted", "packages/a/src/a.ts|react"),
                ("files", "packages/a/src/orphan.ts"),
            ] {
                assert_eq!(classifier.classify(Side::Skif, category, key), None, "{category} {key}");
            }
            assert_eq!(
                classifier.classify(Side::Tool, "dependencies", "examples/app/package.json|left-pad"),
                None,
                "knip does not report on a manifest it does not judge"
            );
        });
    }

    #[test]
    fn knip_only_findings_in_files_below_non_member_projects_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a" })),
            ("packages/a/vendor/lib", json!({ "name": "vendored" })),
            (
                "examples/app",
                json!({ "name": "app", "devDependencies": { "@mastra/core": "1" } }),
            ),
            ("examples/app/esm", json!({ "type": "module" })),
        ];
        with_classifier(&["packages/a"], &manifests, &[], |classifier| {
            for key in [
                "examples/app/src/index.ts",
                "examples/app/esm/index.ts",
                "packages/a/vendor/lib/index.js",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "files", key),
                    Some(NON_MEMBER_PROJECT),
                    "{key}"
                );
            }
            assert_eq!(
                classifier.classify(Side::Tool, "unlisted", "examples/app/esm/a.ts|@mastra/core/agent"),
                Some(NON_MEMBER_PROJECT),
                "the project's own manifest lists the package"
            );
            for (category, key) in [
                ("exports", "examples/app/src/a.ts|unused"),
                ("types", "examples/app/src/a.ts|Unused"),
                ("nsExports", "examples/app/src/a.ts|helper"),
                ("nsTypes", "examples/app/src/a.ts|Helper"),
                ("enumMembers", "examples/app/src/a.ts|Red"),
                ("namespaceMembers", "examples/app/src/a.ts|run"),
                ("duplicates", "examples/app/src/a.ts|a|b"),
                ("unresolved", "examples/app/src/a.ts|../../types/cache"),
                ("unresolved", "packages/a/vendor/lib/index.js|./gone"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, key),
                    Some(NON_MEMBER_PROJECT),
                    "{category} {key}"
                );
            }
            for (category, key) in [
                ("unlisted", "examples/app/src/a.ts|zod"),
                ("dependencies", "examples/app/package.json|left-pad"),
                ("binaries", "examples/app/package.json|tsc"),
                ("exports", "packages/a/src/a.ts|unused"),
                ("unresolved", "scripts/a.ts|./gone"),
                ("files", "packages/a/src/orphan.ts"),
                ("files", "scripts/orphan.ts"),
            ] {
                assert_eq!(classifier.classify(Side::Tool, category, key), None, "{key}");
            }
            assert_eq!(
                classifier.classify(Side::Skif, "files", "examples/app/src/index.ts"),
                Some(NON_MEMBER_PROJECT),
                "a recursive run lints the project with its own entry files"
            );
            assert_eq!(
                classifier.classify(Side::Skif, "exports", "examples/app/src/a.ts|unused"),
                None,
                "knip judges the exports of the project from the enclosing workspace"
            );
        });
    }

    #[test]
    fn in_file_findings_in_a_project_a_recursive_run_lints_as_a_root_stay_compared() {
        let skif_doc = json!({ "diagnostics": [], "roots": [
            { "path": ".", "packages": [".", "packages/a"] },
            { "path": "examples/app", "packages": ["examples/app", "examples/app/plugins/x/"] },
        ]});
        let roots = skif_root_packages(&skif_doc);
        assert_eq!(
            roots,
            strings(&["", "examples/app", "examples/app/plugins/x", "packages/a"])
        );
        assert!(skif_root_packages(&json!({ "diagnostics": [] })).is_empty());

        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a" })),
            (
                "examples/app",
                json!({ "name": "app", "dependencies": { "react": "1" } }),
            ),
            ("examples/other", json!({ "name": "other" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["packages/a"]), &manifest, &every_file);
        let classifier = Classifier {
            skif_root_packages: &roots,
            ..bare(&layout)
        };
        for (category, key) in [
            ("exports", "examples/app/src/a.ts|unused"),
            ("types", "examples/app/src/a.ts|Unused"),
            ("nsExports", "examples/app/src/a.ts|helper"),
            ("nsTypes", "examples/app/src/a.ts|Helper"),
            ("enumMembers", "examples/app/src/a.ts|Red"),
            ("namespaceMembers", "examples/app/src/a.ts|run"),
            ("duplicates", "examples/app/src/a.ts|a|b"),
        ] {
            assert_eq!(classifier.classify(Side::Tool, category, key), None, "{category} {key}");
            assert_eq!(
                classifier.classify(Side::Tool, category, &key.replace("examples/app", "examples/other")),
                Some(NON_MEMBER_PROJECT),
                "{category}: a project that is no root of the run"
            );
        }
        for category in ["exports", "types", "nsExports", "nsTypes"] {
            assert_eq!(
                classifier.classify(Side::Tool, category, "examples/app/src/index.ts|getStaticProps"),
                Some(NON_MEMBER_PROJECT),
                "{category}: the exports of the project's entry are public in skif's run of it"
            );
        }
        assert_eq!(
            classifier.classify(Side::Tool, "enumMembers", "examples/app/index.ts|Red"),
            None
        );
        assert_eq!(
            classifier.classify(Side::Tool, "duplicates", "examples/app/index.ts|a|b"),
            Some(ENTRY_MODULE_DUPLICATE)
        );
        for (category, key) in [
            ("files", "examples/app/src/orphan.ts"),
            ("unresolved", "examples/app/src/a.ts|./gone"),
            ("unlisted", "examples/app/src/a.ts|react"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(NON_MEMBER_PROJECT),
                "{category} {key}"
            );
        }
    }

    #[test]
    fn knip_only_findings_below_directories_skif_discovery_skips_split_off() {
        let manifests = [
            ("", json!({ "name": "root" })),
            ("packages/a", json!({ "name": "a" })),
            (".tools", json!({ "name": "tools" })),
            (".vscode/extensions/helper", json!({ "name": "helper" })),
        ];
        let members = ["packages/a", ".tools", ".vscode/extensions/helper"];
        with_classifier(&members, &manifests, &[], |classifier| {
            for (category, key) in [
                ("files", ".claude/skills/template/index.ts"),
                ("files", ".vscode/extensions/stray.ts"),
                ("files", ".vscode/extensions/helper/.cache/x.ts"),
                ("files", "app/.vercel/output/functions/index.func/index.js"),
                ("files", "out/main.js"),
                ("files", "packages/a/coverage/report.js"),
                ("files", "packages/a/src/dist/bundle.js"),
                ("unresolved", ".eslint-plugin-local/rule.ts|./missing"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, key),
                    Some(SKIPPED_DIRECTORY),
                    "{key}"
                );
            }
            for key in [
                ".github/scripts/release.ts",
                ".storybook/main.ts",
                ".tools/index.ts",
                ".vscode/extensions/helper/src/extension.ts",
                "packages/a/src/out/emit.ts",
                "packages/a/src/target.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
        });
    }

    #[test]
    fn typescript_sources_of_published_paths_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "bin": { "tool": "./cli.js" } })),
            (
                "packages/core",
                json!({ "name": "core", "exports": {
                    ".": { "types": "./index.d.ts", "default": "./bundles/index.js" },
                    "./schematics/*": { "default": "./schematics/*.js" },
                    "./*": "./*",
                } }),
            ),
            (
                "packages/common",
                json!({ "name": "common", "exports": { "./locales/*": { "types": "./locales/*.d.ts" } } }),
            ),
        ];
        with_classifier(&["packages/core", "packages/common"], &manifests, &[], |classifier| {
            for key in [
                "cli.ts",
                "packages/core/index.ts",
                "packages/core/bundles/index.ts",
                "packages/core/schematics/migrations/model-output/index.ts",
                "packages/common/locales/ff-CM.ts",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "files", key),
                    Some(PUBLISHED_SOURCE),
                    "{key}"
                );
            }
            for key in [
                "packages/core/src/other.ts",
                "packages/core/schematics.ts",
                "packages/core/schematics/a.js",
                "packages/common/locales/ff-CM.mts",
                "packages/common/locale/ff-CM.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
            assert!(!classifier.layout.publishes_source_of("packages/core/schematics/types.d.ts"));
            assert_eq!(
                classifier.classify(Side::Tool, "files", "packages/core/schematics/types.d.ts"),
                Some(DECLARATION_FILE)
            );
            for category in ["exports", "types", "nsExports", "nsTypes"] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, "packages/core/index.ts|unused"),
                    Some(PUBLISHED_SOURCE),
                    "the exports of an entry are public: {category}"
                );
                assert_eq!(
                    classifier.classify(Side::Tool, category, "packages/core/src/other.ts|unused"),
                    None,
                    "{category}"
                );
                assert_eq!(
                    classifier.classify(Side::Skif, category, "packages/core/index.ts|unused"),
                    None,
                    "{category}"
                );
            }
            for category in ["enumMembers", "namespaceMembers"] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, "packages/core/index.ts|Red"),
                    None,
                    "skif judges the members of a used export: {category}"
                );
            }
        });
    }

    #[test]
    fn duplicates_only_knip_reports_in_default_entries_and_published_sources_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a", "main": "./dist/server.js" })),
        ];
        with_classifier(&["packages/a"], &manifests, &[], |classifier| {
            for key in [
                "packages/a/src/server.ts|default|format",
                "packages/a/index.mjs|a|b",
                "packages/a/src/index.ts|a|b",
                "packages/a/cli.cts|a|b",
                "src/main.tsx|a|b",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "duplicates", key),
                    Some(ENTRY_MODULE_DUPLICATE),
                    "{key}"
                );
            }
            for (side, category, key) in [
                (Side::Tool, "duplicates", "packages/a/src/util.ts|a|b"),
                (Side::Tool, "duplicates", "packages/a/lib/index.ts|a|b"),
                (Side::Tool, "duplicates", "packages/a/src/main/index.ts|a|b"),
                (Side::Tool, "duplicates", "packages/a/index.json|a|b"),
                (Side::Tool, "exports", "packages/a/src/index.ts|unused"),
                (Side::Skif, "duplicates", "packages/a/src/index.ts|a|b"),
            ] {
                assert_eq!(classifier.classify(side, category, key), None, "{side:?} {category} {key}");
            }
            assert_eq!(
                classifier.classify(Side::Tool, "exports", "packages/a/src/server.ts|unused"),
                Some(PUBLISHED_SOURCE)
            );
        });
    }

    #[test]
    fn published_paths_map_to_their_sources_through_the_build_directories_of_the_package() {
        assert_eq!(build_source("dist", "src", "dist/a/b.js").as_deref(), Some("src/a/b.js"));
        assert_eq!(build_source("dist/esm", "", "dist/esm/a.js").as_deref(), Some("a.js"));
        assert_eq!(build_source("dist", "src", "dist").as_deref(), Some("src"));
        assert_eq!(build_source("dist", "src", "distant/a.js"), None);
        assert_eq!(build_source("", "src", "a.js"), None);

        for (config, found) in [
            (
                "{ // the ESM build\n \"compilerOptions\": { \"outDir\": \"./dist/esm\", \"rootDir\": \"./src\", }, }",
                Some(("dist/esm", "src")),
            ),
            (
                r#"{ "compilerOptions": { "outDir": "dist" }, "include": ["src/**/*.ts", "src/types.d.ts"] }"#,
                Some(("dist", "src")),
            ),
            (
                r#"{ "compilerOptions": { "outDir": "lib" }, "include": ["src/**/*", "scripts/build.ts"] }"#,
                Some(("lib", "")),
            ),
            (
                r#"{ "compilerOptions": { "outDir": "dist" }, "files": ["src/a/index.ts", "src/b.ts"] }"#,
                Some(("dist", "src")),
            ),
            (r#"{ "compilerOptions": { "outDir": "dist" } }"#, None),
            (r#"{ "compilerOptions": { "rootDir": "src" } }"#, None),
            (r#"{ "compilerOptions": { "outDir": "../out", "rootDir": "src" } }"#, None),
            ("{ not json", None),
        ] {
            assert_eq!(
                tsconfig_build_dir(config),
                found.map(|(out_dir, root_dir)| (out_dir.to_owned(), root_dir.to_owned())),
                "{config}"
            );
        }

        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/ai",
                json!({ "name": "ai", "exports": {
                    ".": { "import": "./dist/esm/index.js", "require": "./dist/cjs/index.js" },
                    "./tools": "./dist/esm/tools/index.js",
                } }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let files = strings(&["packages/ai/src/index.ts", "packages/ai/src/tools/index.ts"]);
        let is_file = |file: &str| files.contains(file);
        let build_dirs = |dir: &str| {
            if dir == "packages/ai" {
                vec![
                    ("dist/esm".to_owned(), "src".to_owned()),
                    ("dist/cjs".to_owned(), "src".to_owned()),
                ]
            } else {
                Vec::new()
            }
        };
        let without = Layout::new(strings(&["packages/ai"]), &manifest, &is_file);
        assert!(
            !without.publishes_source_of("packages/ai/src/index.ts"),
            "`dist/esm` maps to `src/esm` by convention alone"
        );
        let layout = Layout::new(strings(&["packages/ai"]), &manifest, &is_file).with_build_dirs(&build_dirs);
        assert!(layout.publishes_source_of("packages/ai/src/index.ts"));
        assert!(layout.publishes_source_of("packages/ai/src/tools/index.ts"));
        let classifier = bare(&layout);
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/ai/src/index.ts"),
            Some(PUBLISHED_SOURCE)
        );
        assert_eq!(
            classifier.classify(Side::Tool, "types", "packages/ai/src/tools/index.ts|ToolOptions"),
            Some(PUBLISHED_SOURCE)
        );
    }

    #[test]
    fn the_harness_reads_the_build_directories_of_a_package_from_its_tsconfig_files() {
        let root = std::env::temp_dir().join(format!("skif-bench-parity-build-dirs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let package = root.join("packages/ai");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("tsconfig.json"), r#"{ "compilerOptions": { "strict": true } }"#).unwrap();
        fs::write(
            package.join("tsconfig.build.esm.json"),
            r#"{ "extends": "./tsconfig.json", "compilerOptions": { "outDir": "dist/esm", "rootDir": "src" } }"#,
        )
        .unwrap();
        fs::write(
            package.join("tsconfig.build.cjs.json"),
            r#"{ "compilerOptions": { "outDir": "dist/cjs" }, "include": ["src"] }"#,
        )
        .unwrap();
        fs::write(package.join("base.json"), r#"{ "compilerOptions": { "outDir": "out", "rootDir": "." } }"#).unwrap();
        let read = build_dir_reader(&root);
        assert_eq!(
            read("packages/ai"),
            vec![
                ("dist/cjs".to_owned(), "src".to_owned()),
                ("dist/esm".to_owned(), "src".to_owned()),
            ],
            "in file name order; a file that is no tsconfig is not read"
        );
        assert!(read("packages/missing").is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn published_paths_below_build_directories_and_without_extensions_map_to_their_sources() {
        let manifests = [
            ("", json!({ "name": "root" })),
            (
                "libraries/heft",
                json!({ "name": "heft", "main": "./lib-commonjs/index.js", "exports": {
                    "./lib/*": { "types": "./lib/*.d.ts", "default": "./lib-commonjs/*.js" },
                    "./package.json": "./package.json",
                } }),
            ),
            (
                "extensions/git",
                json!({ "name": "git", "main": "./out/main", "browser": "./dist/browser/extension" }),
            ),
            (
                "packages/tool",
                json!({ "name": "tool", "bin": { "tool": "./bin/tool" } }),
            ),
            (
                "packages/styles",
                json!({ "name": "styles", "main": "./dist/index.css" }),
            ),
        ];
        let members = ["libraries/heft", "extensions/git", "packages/tool", "packages/styles"];
        with_classifier(&members, &manifests, &[], |classifier| {
            for key in [
                "libraries/heft/src/index.ts",
                "libraries/heft/src/utilities/fs.ts",
                "libraries/heft/src/plugins/copy/CopyFilesPlugin.tsx",
                "extensions/git/src/main.ts",
                "extensions/git/src/browser/extension.ts",
                "packages/tool/bin/tool.ts",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "files", key),
                    Some(PUBLISHED_SOURCE),
                    "{key}"
                );
            }
            for key in [
                "libraries/heft/scripts/build.ts",
                "extensions/git/src/other.ts",
                "extensions/git/main.ts",
                "packages/tool/src/tool.ts",
                "packages/styles/src/index.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
            assert!(
                !classifier.layout.publishes_source_of("libraries/heft/src/index.d.ts"),
                "a declaration file is no source of a published path"
            );
        });
        assert!(is_published_twin("./lib-commonjs/*.js", "src/a/b", "ts"));
        assert!(!is_published_twin("./lib-commonjs/*.js", "lib-commonjs-src/a", "ts"));
        assert!(
            !is_published_twin("./bundles/*.js", "src/index", "ts"),
            "`bundles` is not a build directory"
        );
        assert!(
            !is_published_twin("./out/*", "src/main", "js"),
            "only TypeScript sources"
        );
    }

    #[test]
    fn a_published_path_without_a_star_names_only_the_first_source_that_exists() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/x",
                json!({ "name": "x", "main": "./lib/x.js", "types": "./lib/y.d.ts" }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let both = strings(&["packages/x/src/x.ts", "packages/x/lib/x.ts", "packages/x/lib/y.ts"]);
        let is_file = |file: &str| both.contains(file);
        let layout = Layout::new(strings(&["packages/x"]), &manifest, &is_file);
        assert!(layout.publishes_source_of("packages/x/src/x.ts"));
        assert!(
            !layout.publishes_source_of("packages/x/lib/x.ts"),
            "skif maps `./lib/x.js` to `src/x.ts`, the first candidate that exists"
        );
        assert!(
            layout.publishes_source_of("packages/x/lib/y.ts"),
            "the path itself when no `src` twin exists"
        );
        assert!(
            !layout.publishes_source_of("packages/x/src/y.ts"),
            "a source that does not exist"
        );

        let only_lib = strings(&["packages/x/lib/x.ts", "packages/x/lib/x.tsx"]);
        let is_file = |file: &str| only_lib.contains(file);
        let layout = Layout::new(strings(&["packages/x"]), &manifest, &is_file);
        assert!(layout.publishes_source_of("packages/x/lib/x.ts"));
        assert!(
            !layout.publishes_source_of("packages/x/lib/x.tsx"),
            "`.ts` comes before `.tsx`"
        );
    }

    /// The manifests of a checkout with an n8n node package in `packages/nodes`, by directory.
    fn n8n_manifests() -> impl Fn(&str) -> Option<Rc<Value>> {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/nodes",
                json!({ "name": "nodes", "main": "index.js", "n8n": {
                    "nodes": ["dist/nodes/A/A.node.js"],
                    "credentials": ["dist/credentials/AApi.credentials.js"],
                } }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        move |dir: &str| read.get(dir).cloned()
    }

    #[test]
    fn the_node_and_credential_files_of_an_n8n_node_package_are_published_sources() {
        let manifest = n8n_manifests();
        let files = strings(&[
            "packages/nodes/nodes/A/A.node.ts",
            "packages/nodes/credentials/AApi.credentials.ts",
            "packages/nodes/nodes/A/GenericFunctions.ts",
            "packages/nodes/src/nodes/B/B.node.ts",
        ]);
        let is_file = |file: &str| files.contains(file);
        let layout = Layout::new(strings(&["packages/nodes"]), &manifest, &is_file);
        assert!(
            layout.publishes_source_of("packages/nodes/nodes/A/A.node.ts"),
            "`dist/nodes/A/A.node.js` is built from `nodes/A/A.node.ts`"
        );
        assert!(layout.publishes_source_of("packages/nodes/credentials/AApi.credentials.ts"));
        assert!(
            !layout.publishes_source_of("packages/nodes/nodes/A/GenericFunctions.ts"),
            "a file the package does not list"
        );
        assert!(
            !layout.publishes_source_of("packages/nodes/src/nodes/B/B.node.ts"),
            "a node the package does not list"
        );
    }

    #[test]
    fn files_a_published_source_reaches_through_files_only_knip_reports_unused_are_published_sources() {
        let manifest = n8n_manifests();
        let sources: BTreeMap<&str, &str> = [
            (
                "packages/nodes/nodes/A/A.node.ts",
                "import { request } from './GenericFunctions';\n\
                 import * as fields from './descriptions';\n\
                 import { both } from '../Both';\n\
                 import text from './Raw.ts?raw';\n\
                 import { outside } from '../../../../../outside';\n",
            ),
            (
                "packages/nodes/nodes/A/GenericFunctions.ts",
                "export const request = 1;\n",
            ),
            (
                "packages/nodes/nodes/A/descriptions/index.ts",
                "export * from './Fields.js';\n",
            ),
            ("packages/nodes/nodes/A/descriptions/Fields.ts", "export const a = 1;\n"),
            ("packages/nodes/nodes/A/Raw.ts", "export const raw = 1;\n"),
            ("packages/nodes/nodes/Both.ts", "import { other } from './Other';\n"),
            ("packages/nodes/nodes/Other.ts", "export const other = 1;\n"),
        ]
        .into_iter()
        .collect();
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(strings(&["packages/nodes"]), &manifest, &is_file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let knip_unused: BTreeSet<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let skif_unused = strings(&["packages/nodes/nodes/Both.ts"]);
        assert_eq!(
            published_reach(&knip_unused, &skif_unused, &layout, &read),
            strings(&[
                "packages/nodes/nodes/A/GenericFunctions.ts",
                "packages/nodes/nodes/A/descriptions/Fields.ts",
                "packages/nodes/nodes/A/descriptions/index.ts",
            ]),
            "a file skif reports unused too ends the chain, and a `?raw` import reads no module"
        );
        assert_eq!(
            published_reach(&knip_unused, &knip_unused, &layout, &read),
            BTreeSet::new(),
            "only files knip alone reports unused"
        );
    }

    #[test]
    fn relative_specifiers_resolve_with_extensions_twins_and_index_files() {
        let files = strings(&["src/a.ts", "src/b/index.tsx", "src/c.mts", "index.ts"]);
        let is_file = |file: &str| files.contains(file);
        assert_eq!(resolve_relative("src", "./a", &is_file).as_deref(), Some("src/a.ts"));
        assert_eq!(
            resolve_relative("src/b", "../a.js", &is_file).as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            resolve_relative("src", "./b", &is_file).as_deref(),
            Some("src/b/index.tsx")
        );
        assert_eq!(
            resolve_relative("src", "./c.mjs", &is_file).as_deref(),
            Some("src/c.mts")
        );
        assert_eq!(resolve_relative("src", "..", &is_file).as_deref(), Some("index.ts"));
        assert_eq!(resolve_relative("src", "./missing", &is_file), None);
        assert_eq!(
            resolve_relative("src", "../../a", &is_file),
            None,
            "outside the checkout"
        );
    }

    #[test]
    fn a_star_matches_one_or_more_characters_across_directories() {
        assert!(wildcard_match("schematics/*", "schematics/a/b/index"));
        assert!(wildcard_match("locales/*-x", "locales/ff-x"));
        assert!(!wildcard_match("schematics/*", "schematics/"));
        assert!(!wildcard_match("locales/*-x", "locales/ff-y"));
        assert!(wildcard_match("index", "index"));
        assert_eq!(package_name("@mastra/core/agent"), "@mastra/core");
        assert_eq!(package_name("lodash/fp"), "lodash");
        assert_eq!(package_name("react"), "react");
    }

    #[test]
    fn the_harness_reads_members_and_manifests_from_the_checkout() {
        let root = std::env::temp_dir().join(format!("skif-bench-parity-layout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["packages/a", "packages/b", "examples/app"] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join("package.json"), r#"{"name":"x"}"#).unwrap();
        }
        fs::create_dir_all(root.join("packages/no-manifest")).unwrap();
        fs::create_dir_all(root.join("examples/broken")).unwrap();
        fs::write(root.join("examples/broken/package.json"), "{ not json").unwrap();
        fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n  - '!packages/b'\n",
        )
        .unwrap();
        let root = fs::canonicalize(&root).unwrap();
        assert_eq!(workspace_members(&root), strings(&["packages/a"]));
        let read = manifest_reader(&root);
        assert_eq!(read("examples/app").as_deref(), Some(&json!({ "name": "x" })));
        assert_eq!(
            read("examples/broken").as_deref(),
            Some(&Value::Null),
            "an unparseable manifest is still a manifest"
        );
        assert!(read("packages/no-manifest").is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn findings_inside_a_file_the_other_tool_calls_unused_and_config_errors_split_off_on_both_sides() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            ("packages/cfg", json!({ "name": "cfg" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["packages/cfg"]), &manifest, &every_file);
        let knip_unused = strings(&["examples/a/src/dead.ts"]);
        let skif_unused = strings(&["packages/b/src/gone.ts"]);
        let failed = strings(&["packages/cfg"]);
        let classifier = Classifier {
            knip_failed: &failed,
            knip_unused_files: &knip_unused,
            skif_unused_files: &skif_unused,
            ..bare(&layout)
        };
        for (category, key) in [
            ("exports", "examples/a/src/dead.ts|x"),
            ("types", "examples/a/src/dead.ts|T"),
            ("unlisted", "examples/a/src/dead.ts|react"),
            ("duplicates", "examples/a/src/dead.ts|a|b"),
            ("unresolved", "examples/a/src/dead.ts|./gone"),
        ] {
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(IN_KNIP_UNUSED_FILE),
                "{category} {key}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Tool, "exports", "packages/b/src/gone.ts|x"),
            Some(IN_SKIF_UNUSED_FILE)
        );
        for (side, category, key) in [
            (Side::Skif, "files", "examples/a/src/dead.ts"),
            (Side::Skif, "binaries", "examples/a/src/dead.ts|tsc"),
            (Side::Skif, "exports", "examples/a/src/live.ts|x"),
            (Side::Skif, "exports", "packages/b/src/gone.ts|x"),
            (Side::Tool, "exports", "examples/a/src/dead.ts|x"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
        for side in [Side::Skif, Side::Tool] {
            assert_eq!(
                classifier.classify(side, "exports", "packages/cfg/src/a.ts|x"),
                Some(KNIP_CONFIG_ERROR),
                "{side:?}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Skif, "exports", "packages/other/a.ts|x"),
            None
        );
    }

    #[test]
    fn the_import_scan_names_the_bare_packages_a_file_imports() {
        let source = r##"import React from "react";
import type { Config } from "@scope/config/types";
import "side-effect/register";
import { join } from "node:path";
import local from "./local";
import internal from "#internal";
export { helper } from "re-exported";
const lazy = await import("lazy-package");
const required = require("required-package/sub");
const files = import.meta.glob("./pages/*.ts");
vi.mock("mocked-package");
"##;
        let packages = imported_packages("src/a.ts", source);
        assert_eq!(
            packages.iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "@scope/config",
                "lazy-package",
                "re-exported",
                "react",
                "required-package",
                "side-effect"
            ]
        );
        assert!(imported_packages("styles/a.css", ".a { color: red }").is_empty());
        assert_eq!(bare_package("https://esm.sh/react"), None);
        assert_eq!(bare_package("C:\\deps\\a.js"), None);
        assert_eq!(bare_package("@scope/name/deep"), Some("@scope/name"));
        assert_eq!(typed_package("node"), "node");
        assert_eq!(typed_package("babel__core"), "@babel/core");
    }

    #[test]
    fn dependencies_only_files_knip_lists_unused_import_split_off() {
        let sources = BTreeMap::from([
            (
                "libraries/a/src/dead.ts",
                "import { x } from 'left-pad';\nimport core from '@babel/core';\n",
            ),
            ("libraries/a/vendor/old.ts", "import 'from-nested';\n"),
            ("scripts/dead.ts", "import 'root-only';\n"),
        ]);
        let unused: BTreeSet<String> = sources
            .keys()
            .map(|file| (*file).to_owned())
            .chain(["libraries/a/src/gone.ts".to_owned()])
            .collect();
        let workspaces = ["libraries/a"];
        let read_source = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let workspace_of = |file: &str| owner_dir(file, &|dir: &str| workspaces.contains(&dir));
        let imports = unused_file_imports(&unused, &BTreeSet::new(), &read_source, &workspace_of);
        assert_eq!(
            imports,
            strings(&[
                "libraries/a/package.json|@babel/core",
                "libraries/a/package.json|from-nested",
                "libraries/a/package.json|left-pad",
                "package.json|root-only",
            ]),
            "a file counts to its workspace, an unreadable one imports nothing"
        );
        assert_eq!(
            unused_file_imports(
                &unused,
                &strings(&["libraries/a/src/dead.ts", "scripts/dead.ts"]),
                &read_source,
                &workspace_of
            ),
            strings(&["libraries/a/package.json|from-nested"]),
            "a file skif calls unused too credits nothing on either side"
        );

        let read: BTreeMap<String, Rc<Value>> =
            [("", json!({ "name": "root" })), ("libraries/a", json!({ "name": "a" }))]
                .into_iter()
                .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
                .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&workspaces), &manifest, &every_file);
        let classifier = Classifier {
            knip_unused_files: &unused,
            knip_unused_imports: &imports,
            ..bare(&layout)
        };
        for (category, key) in [
            ("dependencies", "libraries/a/package.json|left-pad"),
            ("devDependencies", "libraries/a/package.json|from-nested"),
            ("optionalPeerDependencies", "libraries/a/package.json|@babel/core"),
            ("devDependencies", "libraries/a/package.json|@types/babel__core"),
            ("devDependencies", "package.json|root-only"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(USED_IN_KNIP_UNUSED_FILE),
                "{category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "dependencies", "package.json|left-pad"),
            (Side::Tool, "dependencies", "libraries/a/package.json|root-only"),
            (Side::Tool, "devDependencies", "libraries/a/package.json|@types/node"),
            (Side::Tool, "binaries", "libraries/a/package.json|left-pad"),
            (Side::Tool, "unlisted", "libraries/a/src/live.ts|left-pad"),
            (Side::Skif, "dependencies", "libraries/a/package.json|left-pad"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }

        let skif_unused = strings(&["libraries/a/src/dead.ts"]);
        let imports = unused_file_imports(&unused, &skif_unused, &read_source, &workspace_of);
        let both_unused = Classifier {
            skif_unused_files: &skif_unused,
            knip_unused_imports: &imports,
            ..classifier
        };
        assert_eq!(
            both_unused.classify(Side::Tool, "dependencies", "libraries/a/package.json|left-pad"),
            None,
            "a dependency only a file both tools call unused imports stays compared"
        );
        assert_eq!(
            both_unused.classify(Side::Tool, "devDependencies", "libraries/a/package.json|from-nested"),
            Some(USED_IN_KNIP_UNUSED_FILE)
        );
    }

    #[test]
    fn dependencies_only_files_skif_reports_unused_import_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "libraries/a/src/dead.ts",
                "import { x } from 'left-pad';\nimport core from '@babel/core';\n",
            ),
            ("libraries/a/src/both.ts", "import 'both-unused';\n"),
        ]);
        let skif_unused = strings(&["libraries/a/src/dead.ts", "libraries/a/src/both.ts"]);
        let knip_unused = strings(&["libraries/a/src/both.ts"]);
        let read_source = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let workspace_of = |file: &str| owner_dir(file, &|dir: &str| dir == "libraries/a");
        let imports = unused_file_imports(&skif_unused, &knip_unused, &read_source, &workspace_of);
        assert_eq!(
            imports,
            strings(&[
                "libraries/a/package.json|@babel/core",
                "libraries/a/package.json|left-pad"
            ]),
            "only the files skif alone reports unused are scanned"
        );

        let read: BTreeMap<String, Rc<Value>> =
            [("", json!({ "name": "root" })), ("libraries/a", json!({ "name": "a" }))]
                .into_iter()
                .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
                .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["libraries/a"]), &manifest, &every_file);
        let classifier = Classifier {
            knip_unused_files: &knip_unused,
            skif_unused_files: &skif_unused,
            skif_unused_imports: &imports,
            ..bare(&layout)
        };
        for (category, key) in [
            ("dependencies", "libraries/a/package.json|left-pad"),
            ("optionalPeerDependencies", "libraries/a/package.json|@babel/core"),
            ("devDependencies", "libraries/a/package.json|@types/babel__core"),
        ] {
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(USED_IN_SKIF_UNUSED_FILE),
                "{category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Skif, "dependencies", "libraries/a/package.json|both-unused"),
            (Side::Skif, "dependencies", "package.json|left-pad"),
            (Side::Skif, "binaries", "libraries/a/package.json|left-pad"),
            (Side::Tool, "dependencies", "libraries/a/package.json|left-pad"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn exports_only_files_one_tool_lists_unused_import_split_off_through_re_exports() {
        let sources = BTreeMap::from([
            (
                "packages/a/src/dead.ts",
                "import { helper, type Shape } from './util';\n\
                 import * as ns from './ns';\n\
                 import value from './def';\n\
                 import './side';\n\
                 import { deep, alias, own } from './barrel';\n\
                 const lazy = import('./lazy');\n\
                 console.log(helper, ns.read, value, deep, alias, own, lazy);\n",
            ),
            (
                "packages/a/src/both.ts",
                "import { second } from './util';\nconsole.log(second);\n",
            ),
            (
                "packages/a/src/util.ts",
                "export const helper = 1;\nexport const second = 2;\nexport const unusedHelper = 3;\nexport type Shape = { a: number };\n",
            ),
            ("packages/a/src/ns.ts", "export const read = 1;\nexport const unread = 2;\n"),
            ("packages/a/src/def.ts", "export default 1;\nexport const other = 2;\n"),
            ("packages/a/src/side.ts", "export const effect = 1;\n"),
            (
                "packages/a/src/barrel.ts",
                "export * from './impl';\nexport { original as alias } from './other';\nexport const own = 1;\n",
            ),
            (
                "packages/a/src/impl.ts",
                "export const deep = 1;\nexport const own = 2;\nexport const shallow = 3;\n",
            ),
            ("packages/a/src/other.ts", "export const original = 1;\nexport const rest = 2;\n"),
            ("packages/a/src/lazy.ts", "export const anything = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let unused = strings(&["packages/a/src/dead.ts", "packages/a/src/both.ts"]);
        let both = strings(&["packages/a/src/both.ts"]);
        let credits = unused_file_credits(&unused, &both, &is_file, &read);
        assert_eq!(
            credits,
            strings(&[
                "packages/a/src/barrel.ts|alias",
                "packages/a/src/barrel.ts|deep",
                "packages/a/src/barrel.ts|own",
                "packages/a/src/def.ts|default",
                "packages/a/src/impl.ts|deep",
                "packages/a/src/lazy.ts|*",
                "packages/a/src/ns.ts|read",
                "packages/a/src/other.ts|original",
                "packages/a/src/util.ts|Shape",
                "packages/a/src/util.ts|helper",
            ]),
            "a side-effect import uses nothing, an `export *` forwards only names the barrel does not export itself, \
             and a file both tools call unused credits nothing"
        );
        assert!(unused_file_credits(&unused, &unused, &is_file, &read).is_empty());

        let manifest = |dir: &str| (dir.is_empty() || dir == "packages/a").then(|| Rc::new(json!({ "name": "a" })));
        let layout = Layout::new(strings(&["packages/a"]), &manifest, &is_file);
        let classifier = Classifier {
            knip_unused_files: &unused,
            skif_unused_files: &both,
            knip_unused_credits: &credits,
            skif_unused_credits: &credits,
            ..bare(&layout)
        };
        for (category, key) in [
            ("exports", "packages/a/src/util.ts|helper"),
            ("types", "packages/a/src/util.ts|Shape"),
            ("nsExports", "packages/a/src/ns.ts|read"),
            ("exports", "packages/a/src/impl.ts|deep"),
            ("exports", "packages/a/src/lazy.ts|anything"),
            ("duplicates", "packages/a/src/impl.ts|deep|shallow"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(IMPORTED_BY_KNIP_UNUSED_FILE),
                "{category} {key}"
            );
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(IMPORTED_BY_SKIF_UNUSED_FILE),
                "{category} {key}"
            );
        }
        for (category, key) in [
            ("exports", "packages/a/src/util.ts|unusedHelper"),
            ("exports", "packages/a/src/util.ts|second"),
            ("nsExports", "packages/a/src/ns.ts|unread"),
            ("exports", "packages/a/src/impl.ts|own"),
            ("exports", "packages/a/src/impl.ts|shallow"),
            ("exports", "packages/a/src/other.ts|rest"),
            ("exports", "packages/a/src/side.ts|effect"),
            ("enumMembers", "packages/a/src/lazy.ts|Red"),
            ("unlisted", "packages/a/src/lazy.ts|react"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                None,
                "{category} {key}"
            );
        }
    }

    #[test]
    fn starlark_strings_and_linked_dependency_labels_read_as_skif_reads_them() {
        assert_eq!(
            starlark_strings("x = ['a', \"b\"] # \"comment\"\ny = \"\"\"c \"quoted\" d\"\"\" + 'e\\'f' + \"\""),
            ["a", "b", "c \"quoted\" d", "e'f", ""]
        );
        for (label, found) in [
            (":node_modules/esbuild", Some(("packages/app", "esbuild"))),
            ("node_modules/esbuild", Some(("packages/app", "esbuild"))),
            (":node_modules/@types/node", Some(("packages/app", "@types/node"))),
            (":node_modules/typescript/bin/tsc", Some(("packages/app", "typescript"))),
            ("//:node_modules/typescript", Some(("", "typescript"))),
            ("//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
            ("@//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
            ("@@//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
            (
                "@npm//packages/app:esbuild/package_json.bzl",
                Some(("packages/app", "esbuild")),
            ),
            ("@npm//:@babel/cli/package_json.bzl", Some(("", "@babel/cli"))),
            ("@npm//packages/app:node_modules/esbuild", None),
            ("//packages/app:esbuild/package_json.bzl", None),
            ("@npm//:defs.bzl", None),
            ("@npm//:esbuild/dir/package_json.bzl", None),
            (":node_modules", None),
            (":node_modules/@scope", None),
            ("//packages/app/node_modules/esbuild", None),
            ("src/index.ts", None),
            ("@types/node", None),
        ] {
            let expected = found.map(|(package, name)| (package.to_owned(), name.to_owned()));
            assert_eq!(linked_dependency(label, "packages/app"), expected, "{label}");
        }
    }

    #[test]
    fn dependencies_bazel_build_files_credit_and_unlisted_findings_at_them_split_off() {
        let rules_js = "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n";
        let root_files: BTreeMap<&str, &str> = [
            ("MODULE.bazel", rules_js),
            ("e2e/js/MODULE.bazel", rules_js),
            ("e2e/plain/MODULE.bazel", "module(name = \"plain\")\n"),
        ]
        .into_iter()
        .collect();
        let manifests = strings(&[
            "package.json",
            "packages/app/package.json",
            "e2e/js/package.json",
            "e2e/plain/package.json",
        ]);
        let is_file = |file: &str| root_files.contains_key(file) || manifests.contains(file);
        let read = |file: &str| root_files.get(file).map(|source| (*source).to_owned());
        let build_files: Vec<(String, String)> = [
            (
                "packages/app/BUILD.bazel",
                "load(\"@npm//packages/app:esbuild/package_json.bzl\", esbuild = \"bin\")\n\
                 # \":node_modules/commented\" is no label\n\
                 js_library(name = \"lib\", srcs = [\"src/index.ts\"], deps = [\":node_modules/zod\", \
                 \"//:node_modules/typescript\", '//packages/other:node_modules/left-pad'])\n",
            ),
            ("packages/app/src/BUILD", "deps = [\"//packages/app:node_modules/@types/node\"]\n"),
            (
                "tools/BUILD.bazel",
                "deps = [\"//packages/app:node_modules/react\", \"//:node_modules/prettier\"]\n",
            ),
            ("e2e/js/BUILD.bazel", "deps = [\":node_modules/uvu\"]\n"),
            ("e2e/plain/BUILD.bazel", "deps = [\":node_modules/jasmine\"]\n"),
        ]
        .into_iter()
        .map(|(path, source)| (path.to_owned(), source.to_owned()))
        .collect();
        let credits = bazel_credits(&build_files, &is_file, &read);
        assert_eq!(
            credits,
            strings(&[
                "e2e/js/package.json|uvu",
                "package.json|prettier",
                "package.json|typescript",
                "packages/app/package.json|@types/node",
                "packages/app/package.json|esbuild",
                "packages/app/package.json|zod",
            ]),
            "a label of another package's directory, a package without a manifest, a comment and a Bazel root \
             without rules_js credit nothing"
        );

        let manifest = |dir: &str| {
            manifests
                .contains(&joined(dir, "package.json"))
                .then(|| Rc::new(json!({ "name": dir })))
        };
        let layout = Layout::new(strings(&["packages/app"]), &manifest, &is_file);
        let classifier = Classifier {
            bazel_credits: &credits,
            ..bare(&layout)
        };
        for (side, category, key) in [
            (Side::Tool, "devDependencies", "packages/app/package.json|zod"),
            (Side::Tool, "dependencies", "package.json|typescript"),
            (Side::Tool, "optionalPeerDependencies", "packages/app/package.json|esbuild"),
            (Side::Skif, "unlisted", "packages/app/BUILD.bazel|lodash"),
            (Side::Skif, "unlisted", "packages/app/src/BUILD|lodash"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                Some(BAZEL_BUILD_FILE),
                "{side:?} {category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "dependencies", "packages/app/package.json|react"),
            (Side::Skif, "dependencies", "packages/app/package.json|zod"),
            (Side::Tool, "unlisted", "packages/app/BUILD.bazel|lodash"),
            (Side::Skif, "unlisted", "packages/app/src/index.ts|lodash"),
            (Side::Skif, "unlisted", "packages/app/BUILD.bazel.bak|lodash"),
        ] {
            assert_eq!(classifier.classify(side, category, key), None, "{side:?} {category} {key}");
        }
    }

    #[test]
    fn the_harness_reads_the_build_files_of_a_bazel_checkout_where_skif_discovery_looks() {
        let root = std::env::temp_dir().join(format!("skif-bench-parity-bazel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["packages/a/src", "node_modules/x", ".cache", "docs/build"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "BUILD.bazel",
            "packages/a/BUILD",
            "packages/a/src/BUILD.bazel",
            "node_modules/x/BUILD.bazel",
            ".cache/BUILD",
            "docs/build/notes.md",
        ] {
            fs::write(root.join(file), format!("# {file}\n")).unwrap();
        }
        assert!(read_build_files(&root).is_empty(), "no Bazel root file");
        fs::write(root.join("MODULE.bazel"), "bazel_dep(name = \"aspect_rules_js\")\n").unwrap();
        let found = read_build_files(&root);
        assert_eq!(
            found.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
            ["BUILD.bazel", "packages/a/BUILD", "packages/a/src/BUILD.bazel"]
        );
        assert_eq!(found[1].1, "# packages/a/BUILD\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn knip_only_findings_in_the_files_of_a_rush_root_split_off() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("apps/heft", json!({ "name": "@rushstack/heft" })),
            (
                "common/autoinstallers/lint",
                json!({ "name": "lint", "dependencies": { "eslint": "9" } }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["apps/heft"]), &manifest, &every_file);
        let classifier = |rush_root: bool| Classifier {
            rush_root,
            ..bare(&layout)
        };
        let rush = classifier(true);
        for (category, key) in [
            ("files", ".prettierrc.js"),
            ("files", "common/scripts/install-run-rush.js"),
            ("files", "common/config/subspaces/default/.pnpmfile.cjs"),
            ("unlisted", "common/scripts/install-run.js|semver"),
            ("exports", "common/scripts/install-run.js|run"),
        ] {
            assert_eq!(
                rush.classify(Side::Tool, category, key),
                Some(RUSH_ROOT_FILE),
                "{category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "files", "apps/heft/src/orphan.ts"),
            (Side::Tool, "unlisted", "apps/heft/src/a.ts|semver"),
            (Side::Skif, "files", ".prettierrc.js"),
        ] {
            assert_eq!(rush.classify(side, category, key), None, "{side:?} {category} {key}");
        }
        assert_eq!(
            rush.classify(Side::Tool, "files", "common/autoinstallers/lint/index.js"),
            Some(NON_MEMBER_PROJECT),
            "a project of its own below the root is not a root file"
        );
        assert_eq!(
            classifier(false).classify(Side::Tool, "files", ".prettierrc.js"),
            None,
            "only a Rush root"
        );
    }

    #[test]
    fn a_marker_only_manifest_does_not_own_the_findings_below_it() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/a",
                json!({ "name": "a", "exports": { "./esm/*": { "default": "./dist/esm/*.js" } } }),
            ),
            ("packages/a/src/esm", json!({ "type": "module" })),
            ("packages/b", json!({ "name": "b" })),
            ("packages/b/src/esm", json!({ "type": "module" })),
            ("packages/c", json!({ "name": "c" })),
            ("packages/c/src/esm", json!({ "type": "module" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(
            strings(&["packages/a", "packages/b", "packages/c"]),
            &manifest,
            &every_file,
        );
        let not_judged_dirs = BTreeMap::from([(
            "packages/a".to_owned(),
            NotJudgedScope::within(vec!["src/esm/*.stories.ts".to_owned()]),
        )]);
        let knip_failed = strings(&["packages/b"]);
        let classifier = Classifier {
            not_judged: &not_judged_dirs,
            knip_failed: &knip_failed,
            ..bare(&layout)
        };
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/a/src/esm/button.stories.ts"),
            Some(NOT_JUDGED),
            "the narrowed scope is matched relative to the package, not to the marker's directory"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/a/src/esm/index.ts"),
            Some(PUBLISHED_SOURCE),
            "the package publishes the source below the marker"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "exports", "packages/b/src/esm/index.ts|unused"),
            Some(KNIP_CONFIG_ERROR),
            "the config error of the package covers the files below the marker"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/a/src/other.ts"),
            None
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/c/src/esm/coverage/report.ts"),
            None,
            "a marker directory is no package root for `coverage`"
        );
    }

    #[test]
    fn earlier_deviations_take_precedence_over_the_layout_ones() {
        let manifests = [
            ("", json!({ "name": "root" })),
            ("examples/app", json!({ "name": "app" })),
        ];
        let read: BTreeMap<String, Rc<Value>> = manifests
            .iter()
            .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
            .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(BTreeSet::new(), &manifest, &every_file);
        let unread = ["examples/app/docs/".to_owned()];
        let not_judged_dirs = BTreeMap::from([("examples/app".to_owned(), NotJudgedScope::Package)]);
        let classifier = Classifier {
            knip_unread: &unread,
            not_judged: &not_judged_dirs,
            ..bare(&layout)
        };
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/docs/a.ts"),
            Some(KNIP_UNREAD)
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/test/a.ts"),
            Some(TEST_FILE)
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/src/a.ts"),
            Some(NOT_JUDGED),
            "skif names the package it did not judge"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/.cache/a.ts"),
            Some(NOT_JUDGED)
        );
    }
}
