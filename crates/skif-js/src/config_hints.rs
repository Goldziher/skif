//! knip's configuration hints beyond the empty patterns: redundant `entry` and `project` patterns, top-level `entry`
//! and `project` in a monorepo, `project` extensions skif does not analyse or leaves out, and published entry files
//! that do not exist. The `stale-js-config` rule reports them.

use skif_core::HashSet;
use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use serde_json::Value;
use skif_config::Config;
use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::analysis::workspace_key;
use crate::discover::Candidate;
use crate::entries::{Disk, names_no_file};
use crate::resolve::{is_read_file, join_normalized};
use crate::setup::{Setup, WorkspaceKeys};
use crate::specifier::package_name;

/// What a [`ConfigHint`] says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HintKind {
    /// An `entry` pattern naming a file a plugin, the manifest, a tool configuration or a script already makes an
    /// entry (knip's `entry-redundant`).
    EntryRedundant,
    /// A `project` pattern naming an entry file, which is always part of the project (knip's `project-redundant`).
    ProjectRedundant,
    /// The top-level `entry` or `project` of a workspace with several packages, where it applies to the root package
    /// only (knip's `entry-top-level` and `project-top-level`).
    TopLevel,
    /// A `project` pattern selecting an extension skif never analyses (knip's `project-extension-unregistered`).
    ExtensionUnregistered,
    /// `project` patterns that name extensions but not one the package has component files of (knip's
    /// `project-extension-excluded`).
    ExtensionExcluded,
    /// A path `package.json` publishes that names no file (knip's `package-entry`).
    PackageEntry,
}

/// One configuration hint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ConfigHint {
    pub kind: HintKind,
    /// The configuration key (`js.entry`, `js.workspaces."packages/a".project`), or for [`HintKind::PackageEntry`]
    /// the manifest field (`main`, `exports`).
    pub key: String,
    /// The pattern, extension (`.json`) or published path the hint is about.
    pub identifier: String,
    /// The package whose manifest a [`HintKind::PackageEntry`] points at.
    pub package: Option<PackageId>,
}

/// The extensions of single-file components skif analyses that knip only reads through a compiler. A package with
/// files of one of them whose `project` names other extensions only leaves them out of the project.
const COMPONENT_EXTENSIONS: [&str; 3] = ["vue", "svelte", "astro"];

/// What the hints are computed from.
pub(crate) struct HintInputs<'a> {
    pub workspace: &'a Workspace,
    pub config: &'a Config,
    pub setup: &'a Setup,
    pub candidates: &'a [Candidate],
    /// Every entry file, absolute.
    pub entries: &'a HashSet<PathBuf>,
    /// The entry files something besides the configured and default globs makes entries, absolute.
    pub provided: &'a HashSet<PathBuf>,
    /// The file system with gitignored files, for the existence checks.
    pub disk: &'a dyn FileSystem,
}

/// Every hint, in package order and without duplicates (a pattern of a glob `[js.workspaces]` key is reported once).
pub(crate) fn config_hints(inputs: &HintInputs<'_>) -> Vec<ConfigHint> {
    let mut hints: IndexSet<ConfigHint> = top_level_hints(inputs.workspace, inputs.config).collect();
    let keys = &inputs.setup.workspace_keys;
    for (id, package) in inputs.workspace.iter() {
        if let Some(package_setup) = inputs.setup.package(id) {
            let scope = Scope {
                inputs,
                keys,
                id,
                package,
                package_setup,
            };
            scope.redundant(&mut hints);
            // knip judges the project patterns only where they select the project, not where a `tsconfig` does.
            if !inputs.config.js.use_tsconfig_files {
                scope.extensions(&mut hints);
            }
        }
    }
    hints.extend(package_entries(inputs));
    hints.into_iter().collect()
}

/// `entry-top-level` and `project-top-level`.
fn top_level_hints<'a>(workspace: &Workspace, config: &'a Config) -> impl Iterator<Item = ConfigHint> + 'a {
    let several = workspace.iter().count() > 1;
    [("entry", &config.js.entry), ("project", &config.js.project)]
        .into_iter()
        .filter(move |_| several)
        .filter_map(|(name, patterns)| {
            let first = patterns.first()?;
            let more = if patterns.len() > 1 { ", …" } else { "" };
            Some(ConfigHint {
                kind: HintKind::TopLevel,
                key: format!("js.{name}"),
                identifier: format!("[{first}{more}]"),
                package: None,
            })
        })
}

/// One package's patterns, judged against the inputs.
struct Scope<'s, 'a> {
    inputs: &'s HintInputs<'a>,
    keys: &'s WorkspaceKeys,
    id: PackageId,
    package: &'s skif_workspace::Package,
    package_setup: &'s crate::setup::PackageSetup,
}

impl Scope<'_, '_> {
    /// The key a pattern of the list `name` is written under: the top-level one for a root pattern from there.
    fn key(&self, name: &str, pattern: Option<&String>) -> String {
        let js = &self.inputs.config.js;
        let top_level = if name == "entry" { &js.entry } else { &js.project };
        let from_top_level = pattern.map_or_else(
            || self.package_setup.user_patterns.project == js.project,
            |pattern| top_level.contains(pattern),
        );
        if self.package.is_root && from_top_level {
            format!("js.{name}")
        } else {
            workspace_key(self.inputs.config, self.keys, self.package, name)
        }
    }

    fn hint(&self, kind: HintKind, name: &str, pattern: Option<&String>, identifier: String) -> ConfigHint {
        ConfigHint {
            kind,
            key: self.key(name, pattern),
            identifier,
            package: None,
        }
    }

    /// `entry-redundant` and, unless a `tsconfig` selects the project, `project-redundant`.
    fn redundant(&self, hints: &mut IndexSet<ConfigHint>) {
        let user = &self.package_setup.user_patterns;
        let dir = &self.package_setup.dir;
        for pattern in &user.entry {
            if literal_path(dir, pattern).is_some_and(|path| self.inputs.provided.contains(&path)) {
                hints.insert(self.hint(HintKind::EntryRedundant, "entry", Some(pattern), pattern.clone()));
            }
        }
        if self.inputs.config.js.use_tsconfig_files {
            return;
        }
        for pattern in &user.project {
            if literal_path(dir, pattern).is_some_and(|path| self.inputs.entries.contains(&path)) {
                hints.insert(self.hint(HintKind::ProjectRedundant, "project", Some(pattern), pattern.clone()));
            }
        }
    }

    /// `project-extension-unregistered` and `project-extension-excluded`.
    fn extensions(&self, hints: &mut IndexSet<ConfigHint>) {
        let mut named: Vec<String> = Vec::new();
        for pattern in self
            .package_setup
            .user_patterns
            .project
            .iter()
            .filter(|pattern| !pattern.starts_with('!'))
        {
            for extension in pattern_extensions(pattern) {
                let file = format!("x.{extension}");
                if !is_read_file(Path::new(&file)) && !self.inputs.setup.compilers.handles(Path::new(&file)) {
                    let identifier = format!(".{extension}");
                    hints.insert(self.hint(HintKind::ExtensionUnregistered, "project", Some(pattern), identifier));
                }
                named.push(extension.to_ascii_lowercase());
            }
        }
        if named.is_empty() {
            return;
        }
        for extension in COMPONENT_EXTENSIONS
            .iter()
            .filter(|extension| !named.iter().any(|n| n == *extension))
        {
            let suffix = format!(".{extension}");
            let has_files =
                self.inputs.candidates.iter().any(|candidate| {
                    candidate.owner == self.id && candidate.rel.to_ascii_lowercase().ends_with(&suffix)
                });
            if has_files {
                hints.insert(self.hint(HintKind::ExtensionExcluded, "project", None, suffix));
            }
        }
    }
}

/// The absolute path a pattern names when it is a plain path rather than a glob, relative to `dir`.
fn literal_path(dir: &Path, pattern: &str) -> Option<PathBuf> {
    let body = pattern.trim();
    let body = body.strip_suffix('!').unwrap_or(body);
    if body.is_empty() || body.starts_with('!') || body.starts_with("../") || body.contains(['*', '?', '[', '{']) {
        return None;
    }
    Some(join_normalized(dir, body.trim_start_matches("./")))
}

/// The extensions, without the dot, that the last segment of a glob selects: `**/*.ts`, `*.{ts,json}`,
/// `*.@(md|mdx)` and `*.json!` (knip's `extractPatternExtensions`).
fn pattern_extensions(pattern: &str) -> Vec<String> {
    let body = pattern.strip_suffix('!').unwrap_or(pattern);
    let segment = body.rsplit('/').next().unwrap_or(body);
    let simple = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric());
    let collect = |inner: &str, delimiter: char| -> Vec<String> {
        inner
            .split(delimiter)
            .map(str::trim)
            .filter(|part| simple(part))
            .map(str::to_owned)
            .collect()
    };
    if let Some(open) = segment.strip_suffix('}') {
        return open
            .rfind(".{")
            .map(|start| &open[start + 2..])
            .filter(|inner| !inner.contains(['{', '}']))
            .map(|inner| collect(inner, ','))
            .unwrap_or_default();
    }
    if let Some(open) = segment.strip_suffix(')') {
        let Some(start) = open.rfind('(') else {
            return Vec::new();
        };
        let before = &open[..start];
        let before = before.strip_suffix(['@', '+', '*', '?']).unwrap_or(before);
        if !before.ends_with('.') || open[start + 1..].contains(['(', ')']) {
            return Vec::new();
        }
        return collect(&open[start + 1..], '|');
    }
    segment
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| simple(extension))
        .map(|extension| vec![extension.to_owned()])
        .unwrap_or_default()
}

/// The published paths of a manifest, with the field that names each, as knip reads them: `main`, `module`,
/// `browser`, `bin`, `types`, `typings` and the leaves of `exports` other than `./*`, with a `*` of an `exports` leaf
/// widened to cross directories.
fn published_paths(manifest: &Value) -> Vec<(&'static str, String)> {
    let mut paths: Vec<(&'static str, String)> = Vec::new();
    let mut push = |field: &'static str, path: &str| {
        let entry = (field, path.to_owned());
        if !path.trim().is_empty() && !paths.contains(&entry) {
            paths.push(entry);
        }
    };
    for field in ["main", "module", "browser"] {
        if let Some(Value::String(path)) = manifest.get(field) {
            push(field, path);
        }
    }
    match manifest.get("bin") {
        Some(Value::String(path)) => push("bin", path),
        Some(Value::Object(bins)) => bins
            .values()
            .filter_map(Value::as_str)
            .for_each(|path| push("bin", path)),
        _ => {}
    }
    for field in ["types", "typings"] {
        if let Some(Value::String(path)) = manifest.get(field) {
            push(field, path);
        }
    }
    if let Some(exports) = manifest.get("exports") {
        let mut leaves = Vec::new();
        export_leaves(exports, &mut leaves);
        for leaf in leaves.into_iter().filter(|leaf| leaf != "./*") {
            push("exports", &widen_stars(&leaf));
        }
    }
    paths
}

fn export_leaves(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Object(map) => map.values().for_each(|item| export_leaves(item, out)),
        _ => {}
    }
}

/// knip's `expandWildcards`: `dir/*` and `dir/*.js` select files in subdirectories too.
fn widen_stars(path: &str) -> String {
    let path = path
        .strip_suffix("/*")
        .map_or_else(|| path.to_owned(), |head| format!("{head}/**"));
    let path = path.replacen("/*.", "/**/*.", 1);
    path.replacen("/*/", "/**/", 1)
}

/// The published paths that name no file ([`names_no_file`]) and are not a dependency's path. A path in `bin` that
/// names no file is reported too, though it should have a source behind it, unless a build step of the package may
/// write it: nothing makes it, so it is a path into nothing (ADR 0016).
fn package_entries(inputs: &HintInputs<'_>) -> Vec<ConfigHint> {
    let disk = Disk {
        fs: inputs.disk,
        gitignore: !inputs.config.js.no_gitignore,
    };
    let mut hints = Vec::new();
    for (id, package) in inputs.workspace.iter() {
        let Some(package_setup) = inputs.setup.package(id) else {
            continue;
        };
        for (field, path) in published_paths(package.manifest.value()) {
            if path.starts_with('!') {
                continue;
            }
            if !path.starts_with(['.', '/'])
                && package_name(&path).is_some_and(|name| package.dependencies.iter().any(|dep| *dep.name == *name))
            {
                continue;
            }
            if names_no_file(
                disk,
                &inputs.workspace.root,
                package_setup,
                package.manifest.value(),
                &path,
            ) {
                hints.push(ConfigHint {
                    kind: HintKind::PackageEntry,
                    key: field.to_owned(),
                    identifier: path,
                    package: Some(id),
                });
            }
        }
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_come_from_the_last_segment() {
        assert_eq!(pattern_extensions("src/**/*.ts"), ["ts"]);
        assert_eq!(pattern_extensions("**/*.{ts,json}!"), ["ts", "json"]);
        assert_eq!(pattern_extensions("docs/*.@(md|mdx)"), ["md", "mdx"]);
        assert_eq!(pattern_extensions("lib/**"), Vec::<String>::new());
        assert_eq!(pattern_extensions("src/{a,b}/x"), Vec::<String>::new());
        assert_eq!(pattern_extensions("index.html"), ["html"]);
    }

    #[test]
    fn literal_paths_are_plain_paths() {
        let dir = Path::new("/ws/a");
        assert_eq!(
            literal_path(dir, "./src/index.ts!"),
            Some(PathBuf::from("/ws/a/src/index.ts"))
        );
        assert_eq!(literal_path(dir, "src/*.ts"), None);
        assert_eq!(literal_path(dir, "!src/a.ts"), None);
        assert_eq!(literal_path(dir, "../b/x.ts"), None);
    }

    #[test]
    fn export_stars_cross_directories() {
        assert_eq!(widen_stars("./dist/*"), "./dist/**");
        assert_eq!(widen_stars("./dist/*.js"), "./dist/**/*.js");
        assert_eq!(widen_stars("./dist/*/index.js"), "./dist/**/index.js");
    }
}
