//! `pnpm-lock.yaml` in lockfile format 9 (pnpm 9 to 12): importers, resolved packages, overrides and patches.
//!
//! From pnpm 11 on the file can hold two YAML documents: the package manager's environment (`configDependencies` and
//! `packageManagerDependencies`, pnpm itself among the packages) and the project's lockfile. Only the project's
//! document is read.
//!
//! Discovery does not read the lockfile; a rule that needs it calls [`PnpmLockfile::read`], so a run whose rules do
//! not pays nothing. Other lockfiles (`yarn.lock`, `package-lock.json`, `bun.lock`) and older pnpm formats are not
//! modelled; [`PnpmLockfile::read`] refuses a pnpm format other than 9.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde_json::{Map, Value};
use skif_core::{DependencyKind, FileSystem, HashMap, LineIndex, Span};

use crate::error::WorkspaceError;
use crate::pnpm::section_index;

/// The file name of pnpm's lockfile.
pub const PNPM_LOCKFILE: &str = "pnpm-lock.yaml";

/// The lockfile names of the other package managers, which [`PnpmLockfile`] does not read.
pub const OTHER_LOCKFILES: [&str; 4] = ["yarn.lock", "package-lock.json", "bun.lock", "bun.lockb"];

/// The only fields of an importer in the document of the package manager's environment.
const ENVIRONMENT_FIELDS: [&str; 2] = ["configDependencies", "packageManagerDependencies"];

/// A parsed `pnpm-lock.yaml`: the project's document of it. Fields it does not model (`settings`, `time`, `catalogs`,
/// resolutions and engines of the packages) are ignored.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PnpmLockfile {
    /// Workspace-relative path.
    pub path: PathBuf,
    /// The file contents as read.
    pub source: Arc<str>,
    /// Offset-to-line/column conversion for `source`.
    pub line_index: Arc<LineIndex>,
    /// `lockfileVersion` as written (`9.0`).
    pub version: String,
    /// The direct dependencies of each importer, by importer directory relative to the lockfile (`.` for the root,
    /// `packages/app`), in file order within an importer.
    pub importers: BTreeMap<String, Vec<LockedDependency>>,
    /// The keys of `packages:` (`name@version`, no peer suffix).
    pub packages: BTreeSet<String>,
    /// `snapshots:` by key (`name@version` with the peer and patch suffix, `react-dom@18.3.1(react@18.3.1)`).
    pub snapshots: BTreeMap<String, Snapshot>,
    /// `patchedDependencies:` by key (`name@version` or `name`) as pnpm recorded them.
    pub patched_dependencies: BTreeMap<String, LockedPatch>,
    /// `overrides:` as pnpm applied them: selector and replacement.
    pub overrides: BTreeMap<String, String>,
    /// The byte range of the project's document in `source`.
    document: Range<usize>,
    /// The entry spans of the sections located by key, each built on its first lookup.
    spans: SectionSpans,
}

/// The spans of the entries of one top-level section, by key.
type SpanIndex = HashMap<Box<str>, Span>;

/// The span indexes of the sections whose entries diagnostics point at: a lookup scans the whole text, so a run that
/// locates many entries scans each section once instead of once per lookup.
#[derive(Debug, Clone, Default)]
struct SectionSpans {
    snapshots: OnceLock<SpanIndex>,
    packages: OnceLock<SpanIndex>,
    importers: OnceLock<SpanIndex>,
    patched_dependencies: OnceLock<SpanIndex>,
}

/// One direct dependency of an importer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LockedDependency {
    /// The name the importer lists (an alias when the specifier is `npm:other@1`).
    pub name: String,
    /// The section it is listed in.
    pub kind: DependencyKind,
    /// The specifier the manifest wrote, when recorded.
    pub specifier: Option<String>,
    /// The resolved version reference: `1.2.3`, `1.2.3(peer@1.0.0)`, `other@1.2.3` for an alias, `link:../lib` for a
    /// workspace package.
    pub version: String,
}

impl LockedDependency {
    /// The key of the snapshot this dependency resolves to; `None` for a `link:` to a workspace directory.
    #[must_use]
    pub fn snapshot_key(&self) -> Option<String> {
        snapshot_key(&self.name, &self.version)
    }
}

/// One resolved package of `snapshots:`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Snapshot {
    /// The package name.
    pub name: String,
    /// The resolved version, without the peer and patch suffix.
    pub version: String,
    /// The `patch_hash=` of the key's suffix: pnpm applied a patch to this resolution.
    pub patch_hash: Option<String>,
    /// The snapshot keys of its `dependencies` and `optionalDependencies`, in file order.
    pub dependencies: Vec<String>,
}

/// One entry of the lockfile's `patchedDependencies:`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct LockedPatch {
    /// The content hash pnpm recorded for the patch file.
    pub hash: Option<String>,
    /// The patch file pnpm recorded (pnpm 9 writes it, pnpm 10 does not).
    pub path: Option<String>,
}

/// The snapshots one importer reaches, each with the dependency that first reached it on a shortest path.
#[derive(Debug, Clone)]
pub struct Resolutions<'l> {
    parents: BTreeMap<&'l str, Option<&'l str>>,
}

impl<'l> Resolutions<'l> {
    /// The reached snapshot keys in byte order.
    pub fn keys(&self) -> impl Iterator<Item = &'l str> + '_ {
        self.parents.keys().copied()
    }

    /// Whether the importer reaches the snapshot `key`.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.parents.contains_key(key)
    }

    /// A shortest chain of snapshot keys from a direct dependency of the importer to `key`, both included; `None`
    /// when the importer does not reach it. Ties go to the dependency listed first.
    #[must_use]
    pub fn chain(&self, key: &str) -> Option<Vec<&'l str>> {
        let (&first, _) = self.parents.get_key_value(key)?;
        let mut chain = vec![first];
        let mut at = first;
        while let Some(&Some(parent)) = self.parents.get(at) {
            chain.push(parent);
            at = parent;
        }
        chain.reverse();
        Some(chain)
    }
}

impl PnpmLockfile {
    /// Reads `pnpm-lock.yaml` from the workspace `root`; `None` when there is none.
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidLockfile`] when the file is not YAML, has an unexpected shape, holds no project
    /// document or a document that is not lockfile format 9, and [`WorkspaceError::Io`] when it exists but cannot be
    /// read.
    pub fn read(fs: &dyn FileSystem, root: &Path) -> Result<Option<Self>, WorkspaceError> {
        let path = root.join(PNPM_LOCKFILE);
        if !fs.is_file(&path) {
            return Ok(None);
        }
        let source = fs
            .read_to_string(&path)
            .map_err(|source| WorkspaceError::Io { path, source })?;
        Self::parse(PathBuf::from(PNPM_LOCKFILE), source).map(Some)
    }

    /// Parses the text of a lockfile; `path` is the workspace-relative path used in errors and diagnostics.
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidLockfile`] as for [`read`](Self::read).
    pub fn parse(path: PathBuf, source: Arc<str>) -> Result<Self, WorkspaceError> {
        let invalid = |message: String| WorkspaceError::InvalidLockfile {
            path: path.clone(),
            message,
        };
        let text = source.strip_prefix('\u{feff}').unwrap_or(&source);
        // The default budget suits configuration files; a lockfile of a large workspace holds millions of nodes. The
        // limits grow with the text, so a document of aliases expanding far beyond its size is still refused.
        let size = text.len();
        let options = serde_saphyr::options! {
            budget: serde_saphyr::budget! {
                max_events: size.saturating_mul(4).max(1_000_000),
                max_nodes: size.saturating_mul(2).max(250_000),
                max_total_scalar_bytes: size.max(64 * 1024 * 1024),
            },
        };
        let mut documents = serde_saphyr::from_str_multiple_with_options::<Value, Vec<Value>>(text, options)
            .map_err(|error| invalid(error.to_string()))?
            .into_iter()
            .map(|document| match document {
                Value::Object(document) => Ok(document),
                _ => Err(invalid("the lockfile is not a mapping".to_owned())),
            })
            .collect::<Result<Vec<_>, _>>()?;
        for document in &documents {
            format_9(document).map_err(invalid)?;
        }
        let project = project_document(&documents).map_err(invalid)?;
        // A document range per parsed document; a null document (`--- ~`), which the parser skips, breaks the
        // correspondence, and the spans are then looked up in the whole text.
        let ranges = document_ranges(&source);
        let document_range = if ranges.len() == documents.len() {
            ranges[project].clone()
        } else {
            0..source.len()
        };
        let mut document = documents.swap_remove(project);
        let version = document.remove("lockfileVersion").map(into_scalar).unwrap_or_default();
        // Each section is taken out of the document, so its keys and strings move into the model instead of being
        // copied.
        let mut mapping = |key: &str| -> Result<Map<String, Value>, WorkspaceError> {
            match document.remove(key) {
                None | Some(Value::Null) => Ok(Map::new()),
                Some(Value::Object(map)) => Ok(map),
                Some(_) => Err(invalid(format!("`{key}` must be a mapping"))),
            }
        };
        let mut importers = BTreeMap::new();
        for (dir, importer) in mapping("importers")? {
            let dependencies =
                locked_dependencies(importer).map_err(|message| invalid(format!("importer `{dir}`: {message}")))?;
            importers.insert(dir, dependencies);
        }
        let packages = mapping("packages")?.into_iter().map(|(key, _)| key).collect();
        let mut snapshots = BTreeMap::new();
        for (key, entry) in mapping("snapshots")? {
            let (name, version, suffix) =
                split_key(&key).ok_or_else(|| invalid(format!("snapshots: `{key}` is not `name@version`")))?;
            let mut dependencies = Vec::new();
            for section in ["dependencies", "optionalDependencies"] {
                if let Some(Value::Object(edges)) = entry.get(section) {
                    dependencies.extend(
                        edges
                            .iter()
                            .filter_map(|(name, version)| snapshot_key(name, &scalar(version))),
                    );
                }
            }
            let snapshot = Snapshot {
                name: name.to_owned(),
                version: version.to_owned(),
                patch_hash: patch_hash(suffix).map(str::to_owned),
                dependencies,
            };
            snapshots.insert(key, snapshot);
        }
        let patched_dependencies = mapping("patchedDependencies")?
            .into_iter()
            .map(|(key, entry)| {
                let locked = match entry {
                    // pnpm 10 records the hash alone.
                    Value::String(hash) => LockedPatch {
                        hash: Some(hash),
                        path: None,
                    },
                    Value::Object(mut fields) => LockedPatch {
                        hash: fields.remove("hash").map(into_scalar).filter(|hash| !hash.is_empty()),
                        path: fields.remove("path").map(into_scalar).filter(|path| !path.is_empty()),
                    },
                    _ => LockedPatch::default(),
                };
                (key, locked)
            })
            .collect();
        let overrides = mapping("overrides")?
            .into_iter()
            .map(|(selector, replacement)| (selector, into_scalar(replacement)))
            .collect();
        Ok(Self {
            line_index: Arc::new(LineIndex::new(&source)),
            path,
            source,
            version,
            importers,
            packages,
            snapshots,
            patched_dependencies,
            overrides,
            document: document_range,
            spans: SectionSpans::default(),
        })
    }

    fn entry_span(&self, index: &OnceLock<SpanIndex>, section: &str, key: &str) -> Option<Span> {
        index
            .get_or_init(|| section_index(&self.source, self.document.clone(), &self.line_index, section))
            .get(key)
            .copied()
    }

    /// The snapshots `importer` reaches through its direct dependencies and theirs. A `link:` to a workspace package
    /// is not followed: that package is an importer of its own.
    #[must_use]
    pub fn resolutions(&self, importer: &str) -> Resolutions<'_> {
        reach(
            &self.snapshots,
            self.importers
                .get(importer)
                .into_iter()
                .flatten()
                .filter_map(LockedDependency::snapshot_key),
        )
    }

    /// The span of the `snapshots:` entry `key`, or of its `packages:` entry when the snapshot is not found.
    #[must_use]
    pub fn snapshot_span(&self, key: &str) -> Option<Span> {
        self.entry_span(&self.spans.snapshots, "snapshots", key).or_else(|| {
            let (name, version, _) = split_key(key)?;
            self.entry_span(&self.spans.packages, "packages", &format!("{name}@{version}"))
        })
    }

    /// The span of the `importers:` entry `importer`.
    #[must_use]
    pub fn importer_span(&self, importer: &str) -> Option<Span> {
        self.entry_span(&self.spans.importers, "importers", importer)
    }

    /// The span of the `patchedDependencies:` entry `key`.
    #[must_use]
    pub fn patched_dependency_span(&self, key: &str) -> Option<Span> {
        self.entry_span(&self.spans.patched_dependencies, "patchedDependencies", key)
    }
}

/// The snapshots reached from the snapshot keys `direct` through the `dependencies` of each, breadth-first, so each
/// has a shortest chain; ties go to the key listed first. A key that names no snapshot is not followed.
pub(crate) fn reach(
    snapshots: &BTreeMap<String, Snapshot>,
    direct: impl IntoIterator<Item = String>,
) -> Resolutions<'_> {
    let mut parents: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    let mut queue = VecDeque::new();
    for key in direct {
        if let Some((key, _)) = snapshots.get_key_value(key.as_str())
            && !parents.contains_key(key.as_str())
        {
            parents.insert(key, None);
            queue.push_back(key.as_str());
        }
    }
    while let Some(key) = queue.pop_front() {
        for dependency in snapshots
            .get(key)
            .into_iter()
            .flat_map(|snapshot| &snapshot.dependencies)
        {
            if let Some((dependency, _)) = snapshots.get_key_value(dependency.as_str())
                && !parents.contains_key(dependency.as_str())
            {
                parents.insert(dependency, Some(key));
                queue.push_back(dependency.as_str());
            }
        }
    }
    Resolutions { parents }
}

/// Checks that `document` is lockfile format 9.
fn format_9(document: &Map<String, Value>) -> Result<(), String> {
    let version = document.get("lockfileVersion").map(scalar).unwrap_or_default();
    if version.split('.').next() == Some("9") {
        return Ok(());
    }
    let written = if version.is_empty() { "none" } else { &*version };
    Err(format!(
        "lockfileVersion {written} is not supported; skif reads lockfile format 9 (pnpm 9 to 12)"
    ))
}

/// Whether `document` is the document of the package manager's environment that pnpm 11 and later write ahead of the
/// project's: it has importers, and each holds `configDependencies` or `packageManagerDependencies` and nothing else.
fn is_environment(document: &Map<String, Value>) -> bool {
    let Some(Value::Object(importers)) = document.get("importers") else {
        return false;
    };
    !importers.is_empty()
        && importers.values().all(|importer| {
            matches!(importer, Value::Object(fields) if !fields.is_empty()
                && fields.keys().all(|field| ENVIRONMENT_FIELDS.contains(&field.as_str())))
        })
}

/// The index of the project's document among the documents of the lockfile: the only one, or the one that is not the
/// package manager's environment.
fn project_document(documents: &[Map<String, Value>]) -> Result<usize, String> {
    match documents {
        [] => Err("the lockfile is not a mapping".to_owned()),
        [_] => Ok(0),
        _ => {
            let mut projects = documents
                .iter()
                .enumerate()
                .filter(|(_, document)| !is_environment(document))
                .map(|(index, _)| index);
            match (projects.next(), projects.next()) {
                (Some(project), None) => Ok(project),
                (None, _) => Err(format!(
                    "none of the {} YAML documents is the project's lockfile: each holds only the package manager's \
                     environment (`configDependencies`, `packageManagerDependencies`)",
                    documents.len()
                )),
                (Some(_), Some(_)) => Err(format!(
                    "more than one of the {} YAML documents is a project's lockfile; pnpm writes one, after the \
                     document of the package manager's environment",
                    documents.len()
                )),
            }
        }
    }
}

/// The byte ranges of the YAML documents of `source`: the text between the `---` and `...` marker lines, without the
/// markers, leaving out stretches of only blank lines, comments and directives.
fn document_ranges(source: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut has_content = false;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let content = line.trim_start_matches('\u{feff}').trim_end_matches(['\r', '\n']);
        if let Some(rest) = document_marker(content) {
            if has_content {
                ranges.push(start..offset);
            }
            // `--- value` starts a document on the marker line itself.
            start = if rest.is_empty() { offset + line.len() } else { offset };
            has_content = !rest.is_empty();
        } else if !matches!(content.trim_start().chars().next(), None | Some('#' | '%')) {
            has_content = true;
        }
        offset += line.len();
    }
    if has_content {
        ranges.push(start..source.len());
    }
    ranges
}

/// The rest of a `---` or `...` marker line without its comment, trimmed; `None` for any other line.
fn document_marker(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("---").or_else(|| line.strip_prefix("..."))?;
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim();
    Some(if rest.starts_with('#') { "" } else { rest })
}

/// The text of the project's document of a `pnpm-lock.yaml`, for readers that scan its lines rather than parse it:
/// the whole text when it holds one document, else the first document whose importers are not only the package
/// manager's environment, found by the lines as pnpm writes them (block mappings); empty when there is none.
#[must_use]
pub fn pnpm_project_document(source: &str) -> &str {
    let ranges = document_ranges(source);
    if ranges.len() <= 1 {
        return source;
    }
    ranges
        .into_iter()
        .map(|range| &source[range])
        .find(|document| !is_environment_text(document))
        .unwrap_or("")
}

/// [`is_environment`] read from the lines of one document as pnpm writes it: the first indentation under
/// `importers:` holds the importers, the next one their fields.
fn is_environment_text(document: &str) -> bool {
    let mut in_importers = false;
    let mut importer_indent = None;
    let mut field_indent = None;
    // Whether the importer being read has a field yet; `None` before the first importer.
    let mut has_field = None;
    for line in document.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            in_importers = trimmed.trim_end() == "importers:";
            continue;
        }
        if !in_importers {
            continue;
        }
        let at = *importer_indent.get_or_insert(indent);
        if indent == at {
            if has_field == Some(false) {
                return false;
            }
            has_field = Some(false);
            continue;
        }
        if indent < at || *field_indent.get_or_insert(indent) != indent {
            continue;
        }
        let field = trimmed.split_once(':').map_or(trimmed, |(field, _)| field).trim();
        if !ENVIRONMENT_FIELDS.contains(&field) {
            return false;
        }
        has_field = Some(true);
    }
    has_field == Some(true)
}

/// Splits a snapshot or package key into name, version and the suffix of peers and patches:
/// `@a/b@1.0.0(react@18.3.1)` is `@a/b`, `1.0.0`, `(react@18.3.1)`. `None` when the key has no `@version`.
#[must_use]
pub fn split_key(key: &str) -> Option<(&str, &str, &str)> {
    let open = key.find('(').unwrap_or(key.len());
    let (base, suffix) = key.split_at(open);
    // A scoped name starts with `@`; the version separator is the first `@` after it.
    let skip = usize::from(base.starts_with('@'));
    let at = base[skip..].find('@')? + skip;
    let (name, version) = (&base[..at], &base[at + 1..]);
    (!name.is_empty() && !version.is_empty() && !name.contains(':')).then_some((name, version, suffix))
}

/// The snapshot key a dependency `name` with resolved `version` refers to: `name@version`, or `version` itself when it
/// names another package (an alias, `other@1.0.0`). `None` for a `link:` to a workspace directory.
#[must_use]
pub fn snapshot_key(name: &str, version: &str) -> Option<String> {
    if version.is_empty() || version.starts_with("link:") {
        return None;
    }
    if split_key(version).is_some() {
        return Some(version.to_owned());
    }
    Some(format!("{name}@{version}"))
}

/// The hash of a `(patch_hash=…)` group of a key suffix.
fn patch_hash(suffix: &str) -> Option<&str> {
    let start = suffix.find("patch_hash=")? + "patch_hash=".len();
    let rest = &suffix[start..];
    Some(&rest[..rest.find(')').unwrap_or(rest.len())])
}

/// A scalar as text: YAML reads `9.0` and `1` as numbers.
fn scalar(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(text) => Cow::Borrowed(text),
        Value::Number(number) => Cow::Owned(number.to_string()),
        Value::Bool(flag) => Cow::Owned(flag.to_string()),
        _ => Cow::Borrowed(""),
    }
}

/// [`scalar`] of a value that is not needed afterwards: a string is moved, not copied.
fn into_scalar(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => scalar(&other).into_owned(),
    }
}

/// The direct dependencies of one importer entry.
fn locked_dependencies(importer: Value) -> Result<Vec<LockedDependency>, String> {
    let Value::Object(mut importer) = importer else {
        return Ok(Vec::new());
    };
    let mut dependencies = Vec::new();
    for kind in [
        DependencyKind::Dependencies,
        DependencyKind::DevDependencies,
        DependencyKind::OptionalDependencies,
    ] {
        let section = match importer.remove(kind.field()) {
            None | Some(Value::Null) => continue,
            Some(Value::Object(section)) => section,
            Some(_) => return Err(format!("`{}` must be a mapping", kind.field())),
        };
        dependencies.reserve(section.len());
        for (name, entry) in section {
            let (specifier, version) = match entry {
                Value::Object(mut fields) => (
                    fields.remove("specifier").map(into_scalar),
                    fields.remove("version").map(into_scalar).unwrap_or_default(),
                ),
                other => (None, into_scalar(other)),
            };
            dependencies.push(LockedDependency {
                name,
                kind,
                specifier,
                version,
            });
        }
    }
    Ok(dependencies)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A root importer that depends on `mid`, which pulls in `bad` and `left@1.0.0` through an alias; a member that
    /// links the root package and depends on `react-dom` with a peer suffix; a patched `left`.
    const LOCK: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true

overrides:
  left: 1.0.0

patchedDependencies:
  left@1.0.0:
    hash: abc123
    path: patches/left@1.0.0.patch

importers:

  .:
    dependencies:
      mid:
        specifier: ^2.0.0
        version: 2.0.0
    devDependencies:
      other-name:
        specifier: npm:left@1.0.0
        version: left@1.0.0(patch_hash=abc123)

  packages/app:
    dependencies:
      root:
        specifier: workspace:*
        version: link:../..
      react-dom:
        specifier: ^18.3.1
        version: 18.3.1(react@18.3.1)

packages:

  bad@1.0.0:
    resolution: {integrity: sha512-bad}

  left@1.0.0:
    resolution: {integrity: sha512-left}

  mid@2.0.0:
    resolution: {integrity: sha512-mid}

  react@18.3.1:
    resolution: {integrity: sha512-react}

  react-dom@18.3.1:
    resolution: {integrity: sha512-react-dom}
    peerDependencies:
      react: ^18.3.1

snapshots:

  bad@1.0.0: {}

  left@1.0.0(patch_hash=abc123): {}

  mid@2.0.0:
    dependencies:
      bad: 1.0.0
    optionalDependencies:
      left: 1.0.0(patch_hash=abc123)

  react-dom@18.3.1(react@18.3.1):
    dependencies:
      react: 18.3.1

  react@18.3.1: {}
";

    fn lock() -> PnpmLockfile {
        PnpmLockfile::parse(PathBuf::from(PNPM_LOCKFILE), Arc::from(LOCK)).unwrap()
    }

    fn slice(span: Option<Span>) -> &'static str {
        let span = span.unwrap();
        &LOCK[span.start as usize..span.end as usize]
    }

    #[test]
    fn importers_list_their_direct_dependencies_with_sections_and_versions() {
        let lock = lock();
        assert_eq!(lock.version, "9.0");
        let root: Vec<_> = lock.importers["."]
            .iter()
            .map(|dependency| (dependency.name.as_str(), dependency.kind, dependency.snapshot_key()))
            .collect();
        assert_eq!(
            root,
            [
                ("mid", DependencyKind::Dependencies, Some("mid@2.0.0".to_owned())),
                (
                    "other-name",
                    DependencyKind::DevDependencies,
                    Some("left@1.0.0(patch_hash=abc123)".to_owned())
                ),
            ]
        );
        let app = &lock.importers["packages/app"];
        assert_eq!(app[0].snapshot_key(), None, "a link is no snapshot");
        assert_eq!(app[1].specifier.as_deref(), Some("^18.3.1"));
        assert_eq!(app[1].snapshot_key().as_deref(), Some("react-dom@18.3.1(react@18.3.1)"));
    }

    #[test]
    fn snapshots_patches_and_overrides_are_read() {
        let lock = lock();
        let left = &lock.snapshots["left@1.0.0(patch_hash=abc123)"];
        assert_eq!((left.name.as_str(), left.version.as_str()), ("left", "1.0.0"));
        assert_eq!(left.patch_hash.as_deref(), Some("abc123"));
        assert_eq!(
            lock.snapshots["mid@2.0.0"].dependencies,
            ["bad@1.0.0", "left@1.0.0(patch_hash=abc123)"]
        );
        assert_eq!(
            lock.patched_dependencies["left@1.0.0"],
            LockedPatch {
                hash: Some("abc123".to_owned()),
                path: Some("patches/left@1.0.0.patch".to_owned())
            }
        );
        assert_eq!(lock.overrides["left"], "1.0.0");
        assert!(lock.packages.contains("react-dom@18.3.1"));
    }

    #[test]
    fn a_pnpm_10_patch_entry_is_the_hash_alone() {
        let text = "lockfileVersion: '9.0'\npatchedDependencies:\n  left@1.0.0: abc123\n";
        let lock = PnpmLockfile::parse(PathBuf::from(PNPM_LOCKFILE), Arc::from(text)).unwrap();
        assert_eq!(lock.patched_dependencies["left@1.0.0"].hash.as_deref(), Some("abc123"));
        assert_eq!(lock.patched_dependencies["left@1.0.0"].path, None);
    }

    #[test]
    fn resolutions_follow_snapshots_but_not_workspace_links_with_shortest_chains() {
        let lock = lock();
        let root = lock.resolutions(".");
        assert_eq!(
            root.keys().collect::<Vec<_>>(),
            ["bad@1.0.0", "left@1.0.0(patch_hash=abc123)", "mid@2.0.0"]
        );
        assert_eq!(root.chain("bad@1.0.0").unwrap(), ["mid@2.0.0", "bad@1.0.0"]);
        // `left` is a direct dependency through an alias as well as a dependency of `mid`: the direct path wins.
        assert_eq!(
            root.chain("left@1.0.0(patch_hash=abc123)").unwrap(),
            ["left@1.0.0(patch_hash=abc123)"]
        );
        let app = lock.resolutions("packages/app");
        assert_eq!(
            app.keys().collect::<Vec<_>>(),
            ["react-dom@18.3.1(react@18.3.1)", "react@18.3.1"]
        );
        assert!(!app.contains("bad@1.0.0"), "the link to the root is not followed");
        assert_eq!(lock.resolutions("missing").keys().count(), 0);
    }

    #[test]
    fn keys_split_into_name_version_and_suffix() {
        assert_eq!(split_key("bad@1.0.0"), Some(("bad", "1.0.0", "")));
        assert_eq!(
            split_key("@types/react@18.3.3(react@18.3.1)"),
            Some(("@types/react", "18.3.3", "(react@18.3.1)"))
        );
        assert_eq!(split_key("1.0.0"), None);
        assert_eq!(split_key("@scope/name"), None);
        assert_eq!(
            snapshot_key("string-width-cjs", "string-width@4.2.3").as_deref(),
            Some("string-width@4.2.3")
        );
        assert_eq!(snapshot_key("lib", "link:../lib"), None);
    }

    #[test]
    fn entries_are_located_at_their_keys() {
        let lock = lock();
        assert_eq!(slice(lock.snapshot_span("bad@1.0.0")), "bad@1.0.0: {}");
        assert_eq!(slice(lock.snapshot_span("mid@2.0.0")), "mid@2.0.0");
        assert_eq!(slice(lock.importer_span("packages/app")), "packages/app");
        assert_eq!(slice(lock.patched_dependency_span("left@1.0.0")), "left@1.0.0");
    }

    #[test]
    fn a_lockfile_that_is_not_format_9_or_not_yaml_is_an_error() {
        let parse = |text: &str| {
            PnpmLockfile::parse(PathBuf::from(PNPM_LOCKFILE), Arc::from(text))
                .unwrap_err()
                .to_string()
        };
        assert!(parse("lockfileVersion: '6.0'\n").contains("lockfileVersion 6.0 is not supported"));
        assert!(parse("importers: {}\n").contains("lockfileVersion none is not supported"));
        assert!(parse("lockfileVersion: '9.0'\nimporters: [a]\n").contains("`importers` must be a mapping"));
        assert!(
            parse("lockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies: [a]\n")
                .contains("importer `.`: `dependencies` must be a mapping")
        );
        let broken = parse("lockfileVersion: '9.0'\nsnapshots:\n  a@1.0.0: {\n");
        assert!(broken.starts_with("invalid \"pnpm-lock.yaml\""), "{broken}");
    }

    #[test]
    fn reading_distinguishes_no_lockfile_from_an_unreadable_one() {
        let fs = skif_core::MemoryFs::new().with_file("/ws/package.json", "{}");
        assert!(PnpmLockfile::read(&fs, Path::new("/ws")).unwrap().is_none());
        let fs = fs.with_file("/ws/pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
        let lock = PnpmLockfile::read(&fs, Path::new("/ws")).unwrap().unwrap();
        assert_eq!(lock.path, Path::new(PNPM_LOCKFILE));
        assert!(lock.importers.is_empty());
    }

    /// The document of the package manager's environment that pnpm 11 and later write ahead of the project's.
    const ENVIRONMENT: &str = "lockfileVersion: '9.0'

importers:

  .:
    configDependencies: {}
    packageManagerDependencies:
      pnpm:
        specifier: 12.9.1
        version: 12.9.1

packages:

  pnpm@12.9.1:
    resolution: {integrity: sha512-pnpm}
    hasBin: true

snapshots:

  pnpm@12.9.1: {}
";

    fn parse(text: &str) -> Result<PnpmLockfile, String> {
        PnpmLockfile::parse(PathBuf::from(PNPM_LOCKFILE), Arc::from(text)).map_err(|error| error.to_string())
    }

    fn assert_same_model(actual: &PnpmLockfile, expected: &PnpmLockfile) {
        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.importers, expected.importers);
        assert_eq!(actual.packages, expected.packages);
        assert_eq!(actual.snapshots, expected.snapshots);
        assert_eq!(actual.patched_dependencies, expected.patched_dependencies);
        assert_eq!(actual.overrides, expected.overrides);
    }

    #[test]
    fn the_project_document_of_a_two_document_lockfile_is_read_in_either_order() {
        let expected = lock();
        for text in [
            format!("---\n{ENVIRONMENT}\n---\n{LOCK}"),
            format!("\u{feff}---\n{ENVIRONMENT}\n---\n{LOCK}"),
            format!("---\n{LOCK}\n---\n{ENVIRONMENT}"),
        ] {
            let lock = parse(&text).unwrap();
            assert_same_model(&lock, &expected);
            assert!(!lock.packages.contains("pnpm@12.9.1"), "{text}");
            assert!(!lock.snapshots.contains_key("pnpm@12.9.1"), "{text}");
            // The spans point into the project's document, not at the environment's `.` importer.
            let project = text.find(LOCK).unwrap();
            let at = |span: Option<Span>| {
                let span = span.unwrap();
                (
                    span.start as usize - project,
                    &text[span.start as usize..span.end as usize],
                )
            };
            assert_eq!(at(lock.importer_span(".")), (LOCK.find("  .:\n").unwrap() + 2, "."));
            assert_eq!(
                at(lock.snapshot_span("bad@1.0.0")),
                (LOCK.find("  bad@1.0.0: {}").unwrap() + 2, "bad@1.0.0: {}")
            );
            assert_eq!(lock.snapshot_span("pnpm@12.9.1"), None, "{text}");
        }
    }

    #[test]
    fn a_single_document_is_read_as_before_with_or_without_a_marker() {
        let expected = lock();
        for text in [format!("---\n{LOCK}"), format!("# lock\n---\n{LOCK}...\n")] {
            assert_same_model(&parse(&text).unwrap(), &expected);
        }
        // A lone document is the project's whatever its shape: the environment alone is read as it is.
        let alone = parse(ENVIRONMENT).unwrap();
        assert_eq!(alone.importers["."].len(), 0);
        assert!(alone.packages.contains("pnpm@12.9.1"));
    }

    #[test]
    fn a_lockfile_without_exactly_one_project_document_is_an_error() {
        let environments = parse(&format!("---\n{ENVIRONMENT}---\n{ENVIRONMENT}")).unwrap_err();
        assert!(
            environments.contains(
                "none of the 2 YAML documents is the project's lockfile: each holds only the package manager's \
                 environment (`configDependencies`, `packageManagerDependencies`)"
            ),
            "{environments}"
        );
        let projects = parse(&format!("---\n{ENVIRONMENT}---\n{LOCK}---\n{LOCK}")).unwrap_err();
        assert!(
            projects.contains("more than one of the 3 YAML documents is a project's lockfile"),
            "{projects}"
        );
        let scalar = parse(&format!("---\n{ENVIRONMENT}---\nplain\n")).unwrap_err();
        assert!(scalar.contains("the lockfile is not a mapping"), "{scalar}");
    }

    #[test]
    fn every_document_must_be_format_9() {
        for text in [
            format!("---\n{}---\n{LOCK}", ENVIRONMENT.replace("'9.0'", "'6.0'")),
            format!("---\n{ENVIRONMENT}---\n{}", LOCK.replace("'9.0'", "'6.0'")),
        ] {
            let error = parse(&text).unwrap_err();
            assert!(
                error.contains("lockfileVersion 6.0 is not supported; skif reads lockfile format 9 (pnpm 9 to 12)"),
                "{error}"
            );
        }
    }

    #[test]
    fn the_environment_document_is_told_by_the_fields_of_its_importers() {
        let cases = [
            (".:\n    configDependencies: {}\n", true),
            (
                ".:\n    packageManagerDependencies:\n      pnpm:\n        specifier: 12.9.1\n",
                true,
            ),
            (
                ".:\n    configDependencies: {}\n  packages/tool:\n    configDependencies:\n      a: 1.0.0\n",
                true,
            ),
            (".: {}\n", false),
            (
                ".:\n    configDependencies: {}\n    dependencies:\n      a:\n        version: 1.0.0\n",
                false,
            ),
            (".:\n    configDependencies: {}\n  packages/app: {}\n", false),
            (
                "packages/app:\n    devDependencies:\n      a:\n        version: 1.0.0\n",
                false,
            ),
        ];
        for (importers, environment) in cases {
            let text = format!("lockfileVersion: '9.0'\nimporters:\n  {importers}packages: {{}}\n");
            let document = serde_saphyr::from_str::<Value>(&text)
                .unwrap()
                .as_object()
                .cloned()
                .unwrap();
            assert_eq!(is_environment(&document), environment, "parsed: {text}");
            assert_eq!(is_environment_text(&text), environment, "lines: {text}");
        }
        let no_importers = "lockfileVersion: '9.0'\npackages: {}\n";
        assert!(!is_environment_text(no_importers));
        assert!(!is_environment(&Map::new()));
    }

    #[test]
    fn documents_are_split_at_their_marker_lines() {
        fn ranges(text: &str) -> Vec<&str> {
            document_ranges(text).into_iter().map(|range| &text[range]).collect()
        }
        assert_eq!(ranges("a: 1\n"), ["a: 1\n"]);
        assert_eq!(ranges("\u{feff}---\na: 1\n---\nb: 2\n"), ["a: 1\n", "b: 2\n"]);
        assert_eq!(
            ranges("# head\n%YAML 1.2\n--- # first\na: 1\n...\n---\r\nb: 2\n--- c: 3\n"),
            ["a: 1\n", "b: 2\n", "--- c: 3\n"]
        );
        assert_eq!(ranges("a: '---'\n----: 1\n"), ["a: '---'\n----: 1\n"]);
        assert_eq!(ranges("---\n# nothing\n\n"), Vec::<&str>::new());
    }

    #[test]
    fn line_readers_get_the_project_document() {
        let two = format!("---\n{ENVIRONMENT}\n---\n{LOCK}");
        assert_eq!(pnpm_project_document(&two), LOCK);
        let reversed = format!("---\n{LOCK}---\n{ENVIRONMENT}");
        assert_eq!(pnpm_project_document(&reversed), LOCK);
        assert_eq!(pnpm_project_document(LOCK), LOCK);
        assert_eq!(pnpm_project_document(ENVIRONMENT), ENVIRONMENT);
        assert_eq!(
            pnpm_project_document(&format!("---\n{ENVIRONMENT}---\n{ENVIRONMENT}")),
            ""
        );
    }
}
