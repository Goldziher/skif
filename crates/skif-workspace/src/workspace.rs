//! Workspace discovery: root manifest, patterns, members, catalogs and package manager.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use skif_config::{Config, PackageManager};
use skif_core::{LineIndex, PathSeg, Span};

use crate::catalogs::{self, Catalogs};
use crate::detect;
use crate::error::WorkspaceError;
use crate::fs::{FileSystem, OsFs};
use crate::package::Package;
use crate::pattern::{self, Expander, Pattern};
use crate::pnpm::{self, YamlSource};

const MANIFEST: &str = "package.json";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

/// Where the workspace patterns were declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSource {
    /// `workspaces: [...]` in the root `package.json`.
    PackageJsonWorkspaces,
    /// `workspaces: { packages: [...] }` in the root `package.json` (yarn nohoist, bun).
    YarnNohoistWorkspaces,
    /// `packages:` in `pnpm-workspace.yaml`.
    PnpmWorkspaceYaml,
}

/// What one workspace pattern matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternFact {
    /// The pattern as written, including a leading `!` for negations.
    pub pattern: String,
    /// Whether the pattern is a negation.
    pub negated: bool,
    /// Directories the pattern selected. For a negation, the positively matched directories it removed.
    pub matched: usize,
    /// Where the pattern is written, when known.
    pub span: Option<Span>,
    /// Workspace-relative file declaring the pattern.
    pub file: PathBuf,
}

/// Observations about discovery; judging them is up to rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryFacts {
    /// Where the patterns came from.
    pub source: WorkspaceSource,
    /// One entry per declared pattern, in declaration order.
    pub patterns: Vec<PatternFact>,
    /// Matched, non-empty, non-hidden directories without a `package.json`, sorted.
    pub dirs_without_manifest: Vec<PathBuf>,
}

/// A discovered workspace.
#[derive(Debug)]
pub struct Workspace {
    /// Absolute workspace root.
    pub root: PathBuf,
    /// Config override, then `packageManager`/`devEngines`, then lockfiles, then `pnpm-workspace.yaml`.
    pub package_manager: Option<PackageManager>,
    /// The root package.
    pub root_package: Package,
    /// Member packages sorted by directory, root excluded.
    pub packages: Vec<Package>,
    /// Catalogs from all sources.
    pub catalogs: Catalogs,
    /// Pattern and directory observations.
    pub facts: DiscoveryFacts,
    /// `pnpm-workspace.yaml` when present.
    pub pnpm_workspace: Option<YamlSource>,
    /// Members whose manifest could not be read or parsed; the rest of the workspace is still discovered.
    pub manifest_errors: Vec<WorkspaceError>,
    ignore_packages: Vec<String>,
}

struct Declared {
    source: WorkspaceSource,
    file: PathBuf,
    patterns: Vec<(String, Option<Span>)>,
}

fn io_error(path: &Path, source: std::io::Error) -> WorkspaceError {
    WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn json_patterns(root_package: &Package) -> Option<Declared> {
    let (source, base, items) = match root_package.manifest.value.get("workspaces")? {
        Value::Array(items) => (WorkspaceSource::PackageJsonWorkspaces, vec![PathSeg::Key("workspaces".into())], items),
        Value::Object(object) => (
            WorkspaceSource::YarnNohoistWorkspaces,
            vec![PathSeg::Key("workspaces".into()), PathSeg::Key("packages".into())],
            object.get("packages")?.as_array()?,
        ),
        _ => return None,
    };
    let patterns = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut pointer = base.clone();
            pointer.push(PathSeg::Index(index));
            Some((item.as_str()?.to_owned(), root_package.manifest.field_span(&pointer)))
        })
        .collect();
    Some(Declared {
        source,
        file: root_package.manifest.path.clone(),
        patterns,
    })
}

impl Workspace {
    /// Discovers the workspace at `root` on the real file system.
    ///
    /// # Errors
    /// See [`WorkspaceError`]; a broken member manifest is not an error but is listed in `manifest_errors`.
    pub fn discover(root: &Path, config: &Config) -> Result<Self, WorkspaceError> {
        Self::discover_with_fs(root, config, Arc::new(OsFs))
    }

    /// Discovers the workspace at `root` through `fs`.
    ///
    /// # Errors
    /// See [`WorkspaceError`]; a broken member manifest is not an error but is listed in `manifest_errors`.
    pub fn discover_with_fs(root: &Path, config: &Config, fs: Arc<dyn FileSystem>) -> Result<Self, WorkspaceError> {
        let fs = fs.as_ref();
        let root = std::path::absolute(root).map_err(|error| io_error(root, error))?;
        if !fs.is_dir(&root) {
            return Err(WorkspaceError::NotADirectory(root));
        }
        let manifest_path = root.join(MANIFEST);
        if !fs.is_file(&manifest_path) {
            return Err(WorkspaceError::MissingRootManifest(root));
        }
        let source = fs.read_to_string(&manifest_path).map_err(|error| io_error(&manifest_path, error))?;
        let root_package = Package::parse(PathBuf::new(), Arc::from(source), true)?;

        let yaml_path = root.join(PNPM_WORKSPACE);
        let (pnpm_workspace, pnpm_config) = if fs.is_file(&yaml_path) {
            let text = fs.read_to_string(&yaml_path).map_err(|error| io_error(&yaml_path, error))?;
            let parsed = pnpm::parse(Path::new(PNPM_WORKSPACE), &text)?;
            let source: Arc<str> = Arc::from(text);
            let line_index = Arc::new(LineIndex::new(&source));
            (
                Some(YamlSource {
                    path: PathBuf::from(PNPM_WORKSPACE),
                    source,
                    line_index,
                }),
                Some(parsed),
            )
        } else {
            (None, None)
        };

        let package_manager = detect::detect(fs, &root, config, &root_package.manifest.value, pnpm_workspace.is_some());

        let from_json = json_patterns(&root_package);
        let from_yaml = |yaml: &YamlSource, packages: &[String]| Declared {
            source: WorkspaceSource::PnpmWorkspaceYaml,
            file: yaml.path.clone(),
            patterns: packages
                .iter()
                .map(|pattern| {
                    (pattern.clone(), pnpm::pattern_span(&yaml.source, &yaml.line_index, pattern))
                })
                .collect(),
        };
        let yaml_declared = pnpm_workspace
            .as_ref()
            .zip(pnpm_config.as_ref().and_then(|parsed| parsed.packages.as_deref()))
            .map(|(yaml, packages)| from_yaml(yaml, packages));
        let chosen = if package_manager == Some(PackageManager::Pnpm) && yaml_declared.is_some() {
            yaml_declared
        } else {
            from_json.or(yaml_declared)
        };
        let declared = chosen.ok_or_else(|| WorkspaceError::NoWorkspaces(root.clone()))?;

        let mut catalogs = pnpm_config.map(|parsed| parsed.catalogs).unwrap_or_default();
        catalogs.merge(catalogs::from_package_json(&root_package.manifest.value, &root_package.manifest.path));

        let expander = Expander { fs, root: &root };
        let mut facts = Vec::new();
        let mut candidates = BTreeSet::new();
        let mut negations = Vec::new();
        for (raw, span) in &declared.patterns {
            let negated = raw.trim_start().starts_with('!');
            let matched = if negated {
                negations.push((facts.len(), pattern::negation(raw)));
                0
            } else {
                let found = expander.expand(&Pattern::new(raw));
                let count = found.len();
                candidates.extend(found);
                count
            };
            facts.push(PatternFact {
                pattern: raw.clone(),
                negated,
                matched,
                span: *span,
                file: declared.file.clone(),
            });
        }
        let mut selected = candidates.clone();
        for (index, matcher) in negations {
            let Some(matcher) = matcher else { continue };
            let hits: Vec<&String> = candidates.iter().filter(|dir| matcher.is_match(dir.as_str())).collect();
            if let Some(fact) = facts.get_mut(index) {
                fact.matched = hits.len();
            }
            for dir in hits {
                selected.remove(dir);
            }
        }

        let mut packages = Vec::new();
        let mut dirs_without_manifest = Vec::new();
        let mut manifest_errors = Vec::new();
        for dir in &selected {
            let manifest = root.join(dir).join(MANIFEST);
            if !fs.is_file(&manifest) {
                // Empty directories linger after switching git branches; they are not worth reporting.
                if fs.read_dir(&root.join(dir)).is_ok_and(|entries| !entries.is_empty()) {
                    dirs_without_manifest.push(PathBuf::from(dir));
                }
                continue;
            }
            match fs.read_to_string(&manifest) {
                Ok(text) => match Package::parse(PathBuf::from(dir), Arc::from(text), false) {
                    Ok(package) => packages.push(package),
                    Err(error) => manifest_errors.push(error),
                },
                Err(error) => manifest_errors.push(io_error(&manifest, error)),
            }
        }

        Ok(Self {
            root,
            package_manager,
            root_package,
            packages,
            catalogs,
            facts: DiscoveryFacts {
                source: declared.source,
                patterns: facts,
                dirs_without_manifest,
            },
            pnpm_workspace,
            manifest_errors,
            ignore_packages: config.workspace.ignore_packages.clone(),
        })
    }

    /// The root first, then the members.
    pub fn all_packages(&self) -> impl Iterator<Item = &Package> {
        std::iter::once(&self.root_package).chain(self.packages.iter())
    }

    /// Looks a package up by its `name`.
    #[must_use]
    pub fn package_by_name(&self, name: &str) -> Option<&Package> {
        self.all_packages().find(|package| package.name.as_deref() == Some(name))
    }

    /// Whether `[workspace] ignore-packages` excludes `package`.
    #[must_use]
    pub fn is_ignored(&self, package: &Package) -> bool {
        self.ignore_packages.iter().any(|pattern| package.matches(pattern))
    }
}
