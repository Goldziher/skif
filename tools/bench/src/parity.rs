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
use skif_js::{in_incomplete_scope, is_out_dir};
use skif_workspace::{OsFs, declared_member_dirs};

use crate::corpus::{self, RUSH_KNIP_ROOT, Repo, Tools};
use crate::diff::{
    self, BASELINE_SCHEMA, Baseline, COMPARED, CategorySet, Counts, Deviation, Entry, Gate, Keys, Samples, Side,
};
use crate::proc::{capture, find_on_path};
use crate::provenance;

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
pub const KNIP_DEVIATIONS: [Deviation; 13] = [
    Deviation {
        id: NON_CODE_FILE,
        reason: "knip lists stylesheets, markup, data and docs as unused files; skif's project set is code files only",
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
        reason: "skif lists the package under `incomplete-packages`: a tool config it cannot read leaves its entry files unknown, so it does not judge the package's dependencies, nor its files, exports, types and members where the config could make entries: the whole package for a `scope` of `\"package\"`, only the files its globs match (relative to the package) for a `scope` of `{\"within\": [...]}` (ADR 0065, ADR 0081)",
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
        reason: "knip reports these findings in a file (unused files, exports, types, members, duplicates and unresolved imports, and unlisted dependencies that the file's own `package.json` lists) below a directory with its own `package.json` (one with fields besides `type`, `sideEffects` and `imports`) that is not one of knip's workspaces (as in `knip-unjudged-manifest`). knip counts such files to an enclosing workspace, without the entry files, plugins and dependencies of their own manifest; skif's analysis of the enclosing workspace skips the directory, and a recursive run lints it as a root of its own when no workspace member encloses it (ADR 0022). The harness checks the manifests, not why skif has no finding for each one",
    },
    Deviation {
        id: SKIPPED_DIRECTORY,
        reason: "knip reports these findings in files below a directory that skif's file discovery skips by name, unless the file lies in a workspace member at or below that directory: `node_modules`, `.git`, `dist`, `.next`, `.turbo`, `.svelte-kit`, `storybook-static`, `.yarn`, `.vercel` and `.nuxt`; `out`, `target` and `coverage` directly in the root or a package directory; and every hidden directory but `.github`, `.storybook`, `.rnstorybook`, `.config`, `.vitepress`, `.vuepress`, `.husky` and `.changeset`. skif reads no file there",
    },
    Deviation {
        id: PUBLISHED_SOURCE,
        reason: "knip reports these TypeScript files as unused, and the package that holds each one publishes its path with a built extension (`.js`, `.mjs`, `.cjs`, `.jsx` or a `.d.ts` variant) or without one in `main`, `module`, `source`, `types`, `typings`, `browser`, `bin` or an `exports` leaf, where `*` matches any part of the path; a path below a build directory (`dist`, `lib`, `out`, `esm` and the like, also `lib-commonjs`) is also the path below `src`. skif makes the source of a published path an entry; knip maps a published path to source only through a tsconfig `outDir`. The harness checks the published path, not why skif has no finding for each file, nor the files only such an entry reaches",
    },
    Deviation {
        id: IN_KNIP_UNUSED_FILE,
        reason: "skif reports these findings (anything but unused files and manifest findings) in a file that knip lists as unused. knip does not analyse an unused file, so it reports nothing inside one; the disagreement is the file, which `files` already counts once",
    },
    Deviation {
        id: IN_SKIF_UNUSED_FILE,
        reason: "knip reports these findings (anything but unused files and manifest findings) in a file that skif reports as unused, so skif reports nothing inside it; the disagreement is the file, which `files` already counts once",
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

/// What skif left unjudged in a package it lists under `incomplete-packages`, from the entry's `scope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotJudgedScope {
    /// `"package"` (or no `scope`): no file of the package is judged.
    Package,
    /// `{"within": [globs]}`: only the files these globs match, relative to the package directory, are not judged.
    Within(Vec<String>),
}

impl NotJudgedScope {
    fn read(value: Option<&Value>) -> Self {
        let globs = value.and_then(|scope| scope.get("within")).and_then(Value::as_array);
        globs.map_or(Self::Package, |globs| {
            Self::Within(globs.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        })
    }

    /// The scope covering both: the whole package when either is (as `skif_core::IncompleteScope::union`).
    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Within(mut globs), Self::Within(more)) => {
                for glob in more {
                    if !globs.contains(&glob) {
                        globs.push(glob);
                    }
                }
                Self::Within(globs)
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

/// The package that owns a finding key (`<path>` or `<path>|<name>`): the deepest directory above the path that
/// `is_package` accepts, the root (`""`) at the latest.
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
/// matches.
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
    let Some(scope) = packages.get(&dir) else {
        return false;
    };
    if !in_files {
        return true;
    }
    match scope {
        NotJudgedScope::Package => true,
        NotJudgedScope::Within(globs) => {
            let path = key_path(key);
            let relative = if dir.is_empty() {
                path
            } else {
                path.strip_prefix(dir.as_str())
                    .and_then(|rest| rest.strip_prefix('/'))
                    .unwrap_or(path)
            };
            in_incomplete_scope(globs, relative)
        }
    }
}

/// The known deviation a finding only skif reports belongs to: an unlisted import that only another package's tsconfig
/// `paths` map (`foreign`, from [`foreign_tsconfig_paths_keys`]), which knip resolves through the merged `paths`.
fn skif_only_deviation(category: &str, key: &str, foreign: &BTreeSet<String>) -> Option<&'static str> {
    (category == "unlisted" && foreign.contains(key)).then_some(FOREIGN_TSCONFIG_PATHS)
}

/// Whether a knip-reported unused file is a known deviation: outside skif's project set (not code), in a directory
/// skif treats as fixtures, a test file or any code file below a test directory (such as the apps of an end-to-end
/// suite). Fixtures and tests only count when `include_test_files` is off.
fn classify_file(path: &str, include_test_files: bool) -> Option<&'static str> {
    let normalized = path.replace('\\', "/");
    let (directories, name) = normalized.rsplit_once('/').unwrap_or(("", normalized.as_str()));
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if !extension.is_some_and(|e| CODE_EXTENSIONS.contains(&e.as_str())) {
        return Some(NON_CODE_FILE);
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
    let name = path.rsplit('/').next().unwrap_or(path);
    if [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
    {
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

/// Whether the source file `stem.extension` (relative to its package) is the twin of the published path `target`: the
/// same path, or for a path below a build directory (`is_out_dir` of `skif-js`: `out`, `lib-commonjs`, ...) the same
/// path below `src`, as skif maps a published path back to its source.
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

/// The file paths a manifest publishes: [`PUBLISHED_FIELDS`], every `bin` and every `exports` leaf but the catch-all
/// `./*`.
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
    paths
}

/// The parsed `package.json` of a directory of the checkout (`""` is the root): `None` when there is none, `Null` when
/// it cannot be read or parsed.
type ManifestReader<'a> = dyn Fn(&str) -> Option<Rc<Value>> + 'a;

/// What the classification reads of a checkout besides the two reports: knip's workspaces and the manifests.
struct Layout<'a> {
    /// The workspace member directories, relative with `/` separators; the root is a workspace without being listed.
    members: BTreeSet<String>,
    manifest: &'a ManifestReader<'a>,
}

impl<'a> Layout<'a> {
    const fn new(members: BTreeSet<String>, manifest: &'a ManifestReader<'a>) -> Self {
        Self { members, manifest }
    }

    fn is_package(&self, dir: &str) -> bool {
        (self.manifest)(dir).is_some()
    }

    /// Whether `dir` holds a project manifest, as skif's file discovery decides it: an unreadable manifest or one with
    /// a field besides [`MARKER_FIELDS`].
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
            let here = !PACKAGE_ROOT_ONLY_DIRECTORIES.contains(&name) || parent.is_empty() || self.is_package(parent);
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

    /// Whether `path` is a TypeScript file whose package publishes its path with a built extension.
    fn publishes_source_of(&self, path: &str) -> bool {
        let Some((stem, extension)) = typescript_source(path) else {
            return false;
        };
        let dir = owner_dir(path, &|dir: &str| self.is_package(dir));
        let Some(manifest) = (self.manifest)(&dir) else {
            return false;
        };
        let relative = if dir.is_empty() {
            Some(stem)
        } else {
            stem.strip_prefix(dir.as_str()).and_then(|rest| rest.strip_prefix('/'))
        };
        relative.is_some_and(|stem| {
            published_paths(&manifest)
                .iter()
                .any(|target| is_published_twin(target, stem, extension))
        })
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
    layout: &'a Layout<'a>,
}

impl Classifier<'_> {
    fn classify(&self, side: Side, category: &str, key: &str) -> Option<&'static str> {
        if self.knip_unread.iter().any(|prefix| key.starts_with(prefix.as_str())) {
            return Some(KNIP_UNREAD);
        }
        let path = key_path(key);
        let is_package = |dir: &str| self.layout.is_package(dir);
        // A configuration knip could not load changes what it reads in the whole package, so both sides are affected.
        let config_error = || {
            self.knip_failed
                .contains(&owner_dir(key, &is_package))
                .then_some(KNIP_CONFIG_ERROR)
        };
        if side == Side::Skif {
            return skif_only_deviation(category, key, self.foreign_paths)
                .or_else(|| self.unjudged_manifest(category, path).then_some(KNIP_UNJUDGED_MANIFEST))
                .or_else(|| in_unused_file(category, path, self.knip_unused_files).then_some(IN_KNIP_UNUSED_FILE))
                .or_else(config_error);
        }
        (category == "files")
            .then(|| classify_file(key, self.include_test_files))
            .flatten()
            .or_else(|| {
                (category != "files" && FIXTURE_CATEGORIES.contains(&category) && in_fixture_directory(path))
                    .then_some(FIXTURE_FILE)
            })
            .or_else(|| not_judged(category, key, self.not_judged, &is_package).then_some(NOT_JUDGED))
            .or_else(|| self.layout.in_skipped_directory(path).then_some(SKIPPED_DIRECTORY))
            .or_else(|| self.non_member_project(category, key).then_some(NON_MEMBER_PROJECT))
            .or_else(|| (category == "files" && self.layout.publishes_source_of(path)).then_some(PUBLISHED_SOURCE))
            .or_else(|| in_unused_file(category, path, self.skif_unused_files).then_some(IN_SKIF_UNUSED_FILE))
            .or_else(config_error)
    }

    /// Whether a finding only skif reports sits on a `package.json` that is not one of knip's workspaces.
    fn unjudged_manifest(&self, category: &str, path: &str) -> bool {
        MANIFEST_CATEGORIES.contains(&category)
            && path
                .strip_suffix("/package.json")
                .is_some_and(|dir| !self.layout.is_workspace(dir))
            && !self.knip_manifests.contains(path)
    }

    /// Whether a finding only knip reports in a file (any category but the manifest ones) lies below a project
    /// directory that is not one of knip's workspaces; an unlisted dependency only when that project's own manifest
    /// lists it, since skif judges the project's imports against that manifest.
    fn non_member_project(&self, category: &str, key: &str) -> bool {
        if MANIFEST_CATEGORIES.contains(&category) {
            return false;
        }
        let Some(dir) = self.layout.non_member_project(key_path(key)) else {
            return false;
        };
        category != "unlisted"
            || key
                .split_once('|')
                .is_some_and(|(_, name)| self.layout.lists(&dir, package_name(name)))
    }
}

/// Whether a finding inside a file (any category but `files` and the manifest ones) sits in a file the other tool
/// reports as unused: a tool analyses nothing inside a file it calls unused, so such a finding follows from the `files`
/// disagreement, which is counted once already.
fn in_unused_file(category: &str, path: &str, unused: &BTreeSet<String>) -> bool {
    category != "files" && !MANIFEST_CATEGORIES.contains(&category) && unused.contains(path)
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

/// Installs the dependencies of a checkout without running lifecycle scripts; `Some(reason)` when that fails.
///
/// # Errors
/// Fails when the install cannot be started or the marker cannot be written.
pub fn install(repo: &Repo, dir: &Path, scratch: &Path, default_timeout: Duration) -> Result<Option<String>> {
    let timeout = repo.install_timeout_secs.map_or(default_timeout, Duration::from_secs);
    let marker = dir.join(INSTALLED_MARKER);
    if is_installed(repo, dir) {
        return Ok(None);
    }
    let (program, args) = match install_command(repo, dir) {
        Ok(command) => command,
        Err(reason) => return Ok(Some(reason)),
    };
    if find_on_path(&program).is_none() {
        return Ok(Some(format!("{program} is not installed")));
    }
    eprintln!("parity: {} {program} {}", repo.name, args.join(" "));
    let run = capture(
        Command::new(&program)
            .args(&args)
            .current_dir(dir)
            .env("CI", "1")
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        scratch,
        "install",
        timeout,
    )?;
    if run.timed_out {
        return Ok(Some(format!(
            "{program} install timed out after {} s",
            timeout.as_secs()
        )));
    }
    if run.code != Some(0) {
        return Ok(Some(format!("{program} install failed: {}", run.failure_summary())));
    }
    fs::write(&marker, &repo.rev).with_context(|| format!("writing {}", marker.display()))?;
    Ok(None)
}

/// Whether the dependencies of a checkout are installed at its pinned commit: its install marker holds the commit.
pub fn is_installed(repo: &Repo, dir: &Path) -> bool {
    fs::read_to_string(dir.join(INSTALLED_MARKER)).is_ok_and(|rev| rev.trim() == repo.rev)
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
    // A repository with `install = false` is compared uninstalled, so a tool that cannot run there says why itself.
    let installs = options.install && repo.install;
    if installs && let Some(reason) = install(repo, &root, &scratch, options.timeout)? {
        return Ok(skipped(format!("install: {reason}")));
    }

    let mut skif_command = Command::new(skif);
    skif_command
        .args([".", "--format", "json", "--no-install"])
        .current_dir(&root);
    let recursive = options.recursive || repo.recursive;
    if recursive {
        skif_command.arg("--recursive");
    }
    if !repo.entry.is_empty() {
        skif_command.arg("--config").arg(write_config(
            &scratch,
            "skif-entry.toml",
            &skif_entry_config(&repo.entry),
        )?);
    }
    let skif_run = capture(&mut skif_command, &scratch, "skif-parity", options.timeout)?;
    if skif_run.timed_out {
        return Ok(skipped("skif timed out"));
    }
    let skif_doc: Value = match serde_json::from_slice(&skif_run.stdout) {
        Ok(doc) => doc,
        Err(error) => return Ok(skipped(format!("skif output is not JSON: {error}"))),
    };
    if skif_doc.get("diagnostics").is_none() {
        let message = skif_doc["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Ok(skipped(format!("skif failed: {message}")));
    }

    let knip_dir = match knip_root(repo, &root, &scratch) {
        Ok(dir) => dir,
        Err(reason) => return Ok(skipped(format!("knip root: {reason}"))),
    };
    let spec = format!("knip@{}", tools.knip);
    let mut knip_command = Command::new("npx");
    knip_command
        .args(["--yes", spec.as_str(), "--reporter", "json"])
        .current_dir(&knip_dir)
        .env("CI", "1")
        .env("DOTENV_CONFIG_QUIET", "true");
    if !options.repo_knip_config {
        let config = write_config(&scratch, "knip-config.json", &knip_config(&repo.entry))?;
        knip_command.arg("--config").arg(config);
    }
    let knip_run = capture(&mut knip_command, &scratch, "knip", options.timeout)?;
    if knip_run.timed_out {
        return Ok(skipped("knip timed out"));
    }
    let knip_doc = match parse_knip_report(&knip_run.stdout) {
        Ok(doc) => doc,
        Err(reason) => {
            return Ok(skipped(format!(
                "knip failed (exit {}): {reason}; stderr: {}",
                knip_run.code.map_or_else(|| "none".to_owned(), |c| c.to_string()),
                knip_run.stderr_summary()
            )));
        }
    };

    let mut skif_found = skif_keys(&skif_doc, &root);
    let knip_found = knip_keys(&knip_doc, &knip_dir);
    align_export_labels(&mut skif_found, &knip_found);
    let unjudged = not_judged_dirs(&skif_doc);
    let foreign_paths = foreign_tsconfig_paths_keys(&skif_doc, &root);
    let knip_errors = knip_config_errors(&knip_run.stderr, &knip_dir);
    let manifest = manifest_reader(&root);
    let layout = Layout::new(workspace_members(&knip_dir), &manifest);
    let knip_failed: BTreeSet<String> = knip_errors
        .iter()
        .map(|file| owner_dir(file, &|dir: &str| layout.is_package(dir)))
        .collect();
    let judged_manifests = knip_manifests(&knip_found);
    let empty = BTreeSet::new();
    let classifier = Classifier {
        knip_unread: &repo.knip_unread,
        include_test_files: options.include_test_files,
        not_judged: &unjudged,
        knip_failed: &knip_failed,
        knip_manifests: &judged_manifests,
        foreign_paths: &foreign_paths,
        knip_unused_files: knip_found.get("files").unwrap_or(&empty),
        skif_unused_files: skif_found.get("files").unwrap_or(&empty),
        layout: &layout,
    };
    let split = diff::split_known(&skif_found, &knip_found, &|side, category, key| {
        classifier.classify(side, category, key)
    });
    let (counts, samples) = diff::compare(&split.skif, &split.tool);
    let installed = is_installed(repo, &root);
    let detail = json!({
        "status": COMPARED,
        "installed": installed,
        "recursive": recursive,
        "skif_rev": skif_rev,
        "knip_exit": knip_run.code,
        "knip_errors": knip_errors,
        "knip_invalid": repo.knip_invalid,
        "known_deviations": split.known,
        "known_samples": split.samples,
        "not_judged_packages": unjudged.keys().collect::<Vec<_>>(),
        "narrowed_packages": unjudged
            .iter()
            .filter_map(|(dir, scope)| match scope {
                NotJudgedScope::Within(globs) => Some((dir.clone(), json!(globs))),
                NotJudgedScope::Package => None,
            })
            .collect::<serde_json::Map<_, _>>(),
        "workspace_members": layout.members.len(),
        "skif_ms": skif_run.elapsed.as_millis(),
        "knip_ms": knip_run.elapsed.as_millis(),
        "skif_total": total(&skif_found),
        "knip_total": total(&knip_found),
        "categories": category_detail(&counts, &samples),
    });
    Ok(Outcome {
        entry: Entry::compared(counts)
            .with_known(split.known)
            .with_tool_errors(knip_errors.len())
            .with_invalid(repo.knip_invalid.clone())
            .with_installed(installed)
            .with_run(recursive, skif_rev),
        detail,
    })
}

/// The directory knip runs in: the checkout, or for `knip-root = "rush"` a scratch clone of it with a throw-away root
/// `package.json` whose `workspaces` are the `projectFolder`s of `rush.json`. knip needs a root manifest and has no Rush
/// support; the checkout itself is never written. `Err` says why the directory could not be made.
fn knip_root(repo: &Repo, root: &Path, scratch: &Path) -> Result<PathBuf, String> {
    match repo.knip_root.as_deref() {
        None => Ok(root.to_path_buf()),
        Some(RUSH_KNIP_ROOT) => {
            let rush =
                fs::read_to_string(root.join("rush.json")).map_err(|error| format!("reading rush.json: {error}"))?;
            let manifest = rush_root_manifest(&rush)?;
            let copy = scratch.join("knip-root");
            if copy.exists() {
                fs::remove_dir_all(&copy).map_err(|error| format!("cleaning {}: {error}", copy.display()))?;
            }
            fs::create_dir_all(scratch).map_err(|error| format!("creating {}: {error}", scratch.display()))?;
            // The checkout is only read: the clone, its objects and the root manifest all live in the scratch directory.
            git_step(
                Command::new("git")
                    .args(["clone", "--quiet", "--no-checkout"])
                    .arg(root)
                    .arg(&copy),
                "clone",
            )?;
            git_step(
                Command::new("git")
                    .arg("-C")
                    .arg(&copy)
                    .args(["checkout", "--quiet", "--detach", repo.rev.as_str()]),
                "checkout",
            )?;
            fs::write(copy.join("package.json"), manifest)
                .map_err(|error| format!("writing the scratch package.json: {error}"))?;
            fs::canonicalize(&copy).map_err(|error| format!("resolving {}: {error}", copy.display()))
        }
        Some(other) => Err(format!("unknown knip-root {other:?}")),
    }
}

/// Runs one git command of the scratch copy; `Err` carries its stderr.
fn git_step(command: &mut Command, step: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("running git {step}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {step} of the scratch copy failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The root `package.json` knip gets for a Rush repository: private, with the `projectFolder` of every project of
/// `rush.json` (JSON with comments) as its `workspaces`.
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
    Ok(json!({ "private": true, "workspaces": folders }).to_string() + "\n")
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
                "| {name} | {status} | {} | {} | {} | {unjudged} | {} |",
                sum(|c| c.skif_only),
                sum(|c| c.tool_only),
                sum(|c| c.both),
                diff::known_total(&entry.known_deviations) - unjudged
            );
        } else {
            let _ = writeln!(
                out,
                "| {name} | skipped: {} | - | - | - | - | - |",
                entry.reason.as_deref().unwrap_or("")
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
    let (tools, repos) = corpus::fetch_suite(&options.corpus_file, "parity", &options.only, &options.corpus_dir)?;
    let tool = format!("knip@{}", tools.knip);
    let skif_rev = provenance::source_rev();
    let mut entries = BTreeMap::new();
    let mut details = serde_json::Map::new();
    for (repo, dir) in &repos {
        eprintln!("parity: {}", repo.name);
        let outcome = parity_repo(options, &skif, &tools, repo, dir, &skif_rev)?;
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
            knip_invalid: None,
            knip_root: None,
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
                (String::new(), NotJudgedScope::Within(vec!["scripts/*.ts".to_owned()])),
                ("packages/api".to_owned(), NotJudgedScope::Package),
                (
                    "packages/web".to_owned(),
                    NotJudgedScope::Within(vec!["src/**/*.stories.tsx".to_owned(), "test/*.ts".to_owned()])
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
            table.contains("| s | compared; knip could not load 2 config files | 1 | 2 | 3 | 0 | 0 |"),
            "{table}"
        );
        assert!(table.contains("`not-judged-by-skif`"), "{table}");
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
        for id in [IN_KNIP_UNUSED_FILE, IN_SKIF_UNUSED_FILE, KNIP_CONFIG_ERROR, NOT_JUDGED] {
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
            json!({ "private": true, "workspaces": ["apps/heft", "libraries/node-core-library"] })
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
    fn knip_runs_in_the_checkout_unless_the_repository_needs_a_synthesized_root() {
        let mut repo: Repo = toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"rush\"\n",
            "a".repeat(40)
        ))
        .unwrap();
        let root = Path::new("/nonexistent/checkout");
        assert_eq!(knip_root(&repo, root, Path::new("/nonexistent/scratch")).unwrap(), root);
        repo.knip_root = Some("lerna".to_owned());
        assert!(
            knip_root(&repo, root, Path::new("/nonexistent/scratch"))
                .unwrap_err()
                .contains("unknown knip-root")
        );
        repo.knip_root = Some(RUSH_KNIP_ROOT.to_owned());
        assert!(
            knip_root(&repo, root, Path::new("/nonexistent/scratch"))
                .unwrap_err()
                .contains("reading rush.json"),
            "the checkout is only read"
        );
    }

    fn strings(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
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
        let layout = Layout::new(strings(members), &manifest);
        let none = BTreeSet::new();
        let judged = strings(judged_by_knip);
        check(&Classifier {
            knip_unread: &[],
            include_test_files: false,
            not_judged: &BTreeMap::new(),
            knip_failed: &none,
            knip_manifests: &judged,
            foreign_paths: &BTreeSet::new(),
            knip_unused_files: &none,
            skif_unused_files: &none,
            layout: &layout,
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
                ("dependencies", "packages/a/package.json|left-pad"),
                ("dependencies", "package.json|left-pad"),
                ("dependencies", "judged/package.json|left-pad"),
                ("unlisted", "examples/app/src/a.ts|react"),
                ("binaries", "examples/app/.github/workflows/ci.yml|turbo"),
            ] {
                assert_eq!(classifier.classify(Side::Skif, category, key), None, "{key}");
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
                None
            );
        });
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
                "packages/core/schematics/types.d.ts",
                "packages/common/locales/ff-CM.mts",
                "packages/common/locale/ff-CM.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
            assert_eq!(
                classifier.classify(Side::Tool, "exports", "packages/core/index.ts|unused"),
                None,
                "only unused files"
            );
        });
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
                "libraries/heft/src/index.d.ts",
                "extensions/git/src/other.ts",
                "extensions/git/main.ts",
                "packages/tool/src/tool.ts",
                "packages/styles/src/index.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
        });
        assert!(is_published_twin("./lib-commonjs/*.js", "src/a/b", "ts"));
        assert!(!is_published_twin("./lib-commonjs/*.js", "lib-commonjs-src/a", "ts"));
        assert!(
            !is_published_twin("./bundles/index.js", "src/index", "ts"),
            "`bundles` is not a build directory"
        );
        assert!(
            !is_published_twin("./out/main", "src/main", "js"),
            "only TypeScript sources"
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
        let layout = Layout::new(BTreeSet::new(), &manifest);
        let knip_unused = strings(&["examples/a/src/dead.ts"]);
        let skif_unused = strings(&["packages/b/src/gone.ts"]);
        let failed = strings(&["packages/cfg"]);
        let none = BTreeSet::new();
        let classifier = Classifier {
            knip_unread: &[],
            include_test_files: false,
            not_judged: &BTreeMap::new(),
            knip_failed: &failed,
            knip_manifests: &none,
            foreign_paths: &none,
            knip_unused_files: &knip_unused,
            skif_unused_files: &skif_unused,
            layout: &layout,
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
        let layout = Layout::new(BTreeSet::new(), &manifest);
        let unread = ["examples/app/docs/".to_owned()];
        let not_judged_dirs = BTreeMap::from([("examples/app".to_owned(), NotJudgedScope::Package)]);
        let none = BTreeSet::new();
        let classifier = Classifier {
            knip_unread: &unread,
            include_test_files: false,
            not_judged: &not_judged_dirs,
            knip_failed: &none,
            knip_manifests: &none,
            foreign_paths: &BTreeSet::new(),
            knip_unused_files: &none,
            skif_unused_files: &none,
            layout: &layout,
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
