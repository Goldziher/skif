//! Bazel with `rules_js`: the `BUILD.bazel` and `BUILD` files of a package are scanned as Starlark text, never
//! evaluated.
//!
//! A label of the `node_modules` that `npm_link_all_packages` links for a package (`":node_modules/esbuild"`,
//! `"//packages/app:node_modules/@types/node"`) and the load of a package's `package_json.bzl` from the npm repository
//! (`"@npm//packages/app:esbuild/package_json.bzl"`) name a dependency of the package whose directory the label's
//! Bazel package is, and the package uses it; only that package may list it, whichever package's `BUILD` file holds
//! the label. The `entry_point` of `js_binary`, `js_run_devserver` and `js_test` is an entry. The `data` of those rules
//! and the `srcs` and `data` of `js_library` and `ts_project` are build sources: used files whose imports are
//! followed, but no entries, so their exports are judged. Each is a file of the package named by its path or by a
//! label of the `BUILD` file's own package (`":src/index.ts"`). A computed value (`glob([...])`, `select({...})`, a
//! variable) names nothing.
//!
//! The plugin reads nothing outside a Bazel repository that uses `rules_js`: the nearest directory at or above the
//! package holding `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` or `WORKSPACE` must name `aspect_rules_js` in
//! one of its root files. The `BUILD` files of a nested package and those in Bazel's output trees (`bazel-bin`,
//! `bazel-out`) give no entries; a nested package's `BUILD` file still names dependencies of this package by label.

use std::path::Path;

use skif_core::slash;

use crate::bazel::{ROOT_FILES, calls, is_output_path, package_path, repository_root, string_literals};
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, package_name};

/// The `bazel` plugin.
pub struct Bazel;

static SPEC: PluginSpec = PluginSpec {
    id: "bazel",
    enablers: &[Enabler::File("BUILD.bazel"), Enabler::File("BUILD")],
    config: &["**/BUILD.bazel", "**/BUILD"],
    ..PluginSpec::DEFAULT
};

/// The text a root file of a Bazel repository that uses `rules_js` holds: its module or repository name.
const RULES_JS: &str = "aspect_rules_js";

/// The rules whose attributes name the files they build from, those attributes, and whether the rule is a test, whose
/// files are no part of a production build.
const RULES: &[(&str, &[&str], bool)] = &[
    ("js_binary", &["entry_point", "data"], false),
    ("js_run_devserver", &["entry_point", "data"], false),
    ("js_test", &["entry_point", "data"], true),
    ("js_library", &["srcs", "data"], false),
    ("ts_project", &["srcs", "data"], false),
];

/// The attribute that names the file a rule runs, an entry whose exports are public. The files every other attribute
/// of [`RULES`] names are build sources.
const ENTRY_POINT: &str = "entry_point";

impl Plugin for Bazel {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// `BUILD` files are Starlark, which no config loader reads: the source is scanned instead.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(root) = repository_root(cx.fs, cx.dir).filter(|root| uses_rules_js(cx, root)) else {
            return Vec::new();
        };
        if is_output_path(&root, config.path) {
            return Vec::new();
        }
        let Some(package) = cx.dir.strip_prefix(&root).ok().map(slash) else {
            return Vec::new();
        };
        let bazel_package = join_rel(&package, config.base);
        let mut out: Vec<PluginInput> = string_literals(config.source)
            .iter()
            .filter_map(|literal| linked_dependency(literal, &bazel_package))
            .filter(|(owner, _)| *owner == package)
            .map(|(_, name)| PluginInput::ReaderDependency(name))
            .collect();
        if in_nested_package(cx, config.base) {
            return out;
        }
        for call in calls(config.source) {
            let Some((_, attributes, test)) = RULES.iter().find(|(rule, _, _)| *rule == call.rule) else {
                continue;
            };
            for attribute in *attributes {
                for path in call.values(attribute).filter_map(package_path) {
                    let file = config.join(path);
                    if cx.fs.is_file(&cx.dir.join(&file)) {
                        out.push(input(file, *attribute == ENTRY_POINT, *test));
                    }
                }
            }
        }
        out
    }
}

/// The input for `file`, which a rule names in its `entry_point` when `entry`, of a test rule when `test`.
fn input(file: String, entry: bool, test: bool) -> PluginInput {
    match (entry, test) {
        (true, true) => PluginInput::Entry(file),
        (true, false) => PluginInput::ProductionEntry(file),
        (false, true) => PluginInput::BuildSource(file),
        (false, false) => PluginInput::ProductionBuildSource(file),
    }
}

/// Whether a root file of the Bazel repository at `root` names `rules_js`.
fn uses_rules_js(cx: &PluginContext<'_>, root: &Path) -> bool {
    ROOT_FILES.iter().any(|name| {
        cx.fs
            .read_to_string(&root.join(name))
            .is_ok_and(|text| text.contains(RULES_JS))
    })
}

/// Whether `base`, a directory relative to the package, lies in another package: it or a directory between it and the
/// package holds a `package.json`.
fn in_nested_package(cx: &PluginContext<'_>, base: &str) -> bool {
    let mut dir = base;
    while !dir.is_empty() {
        if cx.fs.is_file(&cx.dir.join(dir).join("package.json")) {
            return true;
        }
        dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    false
}

/// The Bazel package, relative to the repository root, and the npm package that `label` names as a linked dependency,
/// written in a `BUILD` file of the Bazel package `bazel_package`: a `node_modules/<name>` target of the main
/// repository (`":node_modules/x"`, `"//pkg:node_modules/@scope/x"`, `"@//pkg:node_modules/x/dir"`) or the
/// `<name>/package_json.bzl` of another repository (`"@npm//pkg:x/package_json.bzl"`).
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
        let named = !repository.is_empty() && package_name(name).as_deref() == Some(name);
        return named.then(|| (package.to_owned(), name.to_owned()));
    }
    let linked = target.strip_prefix("node_modules/")?;
    if !repository.is_empty() {
        return None;
    }
    package_name(linked).map(|name| (package.to_owned(), name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_of_linked_packages_name_the_dependency_and_its_bazel_package() {
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
            ("//packages/app/node_modules/esbuild", None),
            ("src/index.ts", None),
            ("@types/node", None),
        ] {
            let expected = found.map(|(package, name)| (package.to_owned(), name.to_owned()));
            assert_eq!(linked_dependency(label, "packages/app"), expected, "{label}");
        }
    }
}
