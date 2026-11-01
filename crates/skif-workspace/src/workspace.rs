//! Workspace discovery: root manifest, patterns, members, catalogs and package manager.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde_json::Value;
use skif_config::{Config, PackageManager};
use skif_core::{FileSystem, LineIndex, OsFs, PathSeg, Span};

use crate::catalogs::{self, Catalogs};
use crate::detect;
use crate::error::WorkspaceError;
use crate::package::{Package, PackageId};
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
    /// Indexed by [`PackageId`]: whether `[workspace] ignore-packages` excludes the package.
    ignored: Vec<bool>,
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

/// The outcome of loading one selected directory.
enum Member {
    Loaded(Box<Package>),
    /// A non-empty directory without a `package.json`.
    NoManifest(PathBuf),
    /// An empty directory; those linger after switching git branches and are not worth reporting.
    Empty,
    Failed(WorkspaceError),
}

fn load_member(fs: &dyn FileSystem, root: &Path, dir: &str) -> Member {
    let manifest = root.join(dir).join(MANIFEST);
    if !fs.is_file(&manifest) {
        return if fs.read_dir(&root.join(dir)).is_ok_and(|entries| !entries.is_empty()) {
            Member::NoManifest(PathBuf::from(dir))
        } else {
            Member::Empty
        };
    }
    match fs.read_to_string(&manifest) {
        Ok(text) => match Package::parse(PathBuf::from(dir), text, false) {
            Ok(package) => Member::Loaded(Box::new(package)),
            Err(error) => Member::Failed(error),
        },
        Err(error) => Member::Failed(io_error(&manifest, error)),
    }
}

fn json_patterns(root_package: &Package) -> Option<Declared> {
    let (source, base, items) = match root_package.manifest.value.get("workspaces")? {
        Value::Array(items) => (
            WorkspaceSource::PackageJsonWorkspaces,
            vec![PathSeg::Key("workspaces".into())],
            items,
        ),
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
        Self::discover_with_fs(root, config, &OsFs)
    }

    /// Discovers the workspace at `root` through `fs`.
    ///
    /// # Errors
    /// See [`WorkspaceError`]; a broken member manifest is not an error but is listed in `manifest_errors`.
    pub fn discover_with_fs(root: &Path, config: &Config, fs: &dyn FileSystem) -> Result<Self, WorkspaceError> {
        let root = std::path::absolute(root).map_err(|error| io_error(root, error))?;
        if !fs.is_dir(&root) {
            return Err(WorkspaceError::NotADirectory(root));
        }
        let manifest_path = root.join(MANIFEST);
        if !fs.is_file(&manifest_path) {
            return Err(WorkspaceError::MissingRootManifest(root));
        }
        let source = fs
            .read_to_string(&manifest_path)
            .map_err(|error| io_error(&manifest_path, error))?;
        let root_package = Package::parse(PathBuf::new(), source, true)?;

        let yaml_path = root.join(PNPM_WORKSPACE);
        let (pnpm_workspace, pnpm_config) = if fs.is_file(&yaml_path) {
            let text = fs
                .read_to_string(&yaml_path)
                .map_err(|error| io_error(&yaml_path, error))?;
            let parsed = pnpm::parse(Path::new(PNPM_WORKSPACE), &text)?;
            let line_index = Arc::new(LineIndex::new(&text));
            (
                Some(YamlSource {
                    path: PathBuf::from(PNPM_WORKSPACE),
                    source: text,
                    line_index,
                }),
                Some(parsed),
            )
        } else {
            (None, None)
        };

        let package_manager = detect::detect(
            fs,
            &root,
            config,
            &root_package.manifest.value,
            pnpm_workspace.is_some(),
        );

        let from_json = json_patterns(&root_package);
        let from_yaml = |yaml: &YamlSource, packages: &[String]| Declared {
            source: WorkspaceSource::PnpmWorkspaceYaml,
            file: yaml.path.clone(),
            patterns: packages
                .iter()
                .map(|pattern| {
                    (
                        pattern.clone(),
                        pnpm::pattern_span(&yaml.source, &yaml.line_index, pattern),
                    )
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
        catalogs.merge(catalogs::from_package_json(
            &root_package.manifest.value,
            &root_package.manifest.path,
        ));

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

        let dirs: Vec<&String> = selected.iter().collect();
        let members: Vec<Member> = dirs.par_iter().map(|dir| load_member(fs, &root, dir)).collect();
        let mut packages = Vec::new();
        let mut dirs_without_manifest = Vec::new();
        let mut manifest_errors = Vec::new();
        for member in members {
            match member {
                Member::Loaded(package) => {
                    let id = PackageId::from_index(packages.len() + 1);
                    packages.push((*package).with_id(id));
                }
                Member::NoManifest(dir) => dirs_without_manifest.push(dir),
                Member::Empty => {}
                Member::Failed(error) => manifest_errors.push(error),
            }
        }
        let ignore = &config.workspace.ignore_packages;
        let ignored: Vec<bool> = std::iter::once(&root_package)
            .chain(&packages)
            .map(|package| package.is_selected_by(ignore))
            .collect();

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
            ignored,
        })
    }

    /// The root first, then the members.
    pub fn all_packages(&self) -> impl Iterator<Item = &Package> {
        std::iter::once(&self.root_package).chain(self.packages.iter())
    }

    /// The ids of every package, the root first.
    pub fn package_ids(&self) -> impl Iterator<Item = PackageId> + use<> {
        (0..=self.packages.len()).map(PackageId::from_index)
    }

    /// Every package with its id, the root first.
    pub fn iter(&self) -> impl Iterator<Item = (PackageId, &Package)> {
        self.all_packages().map(|package| (package.id, package))
    }

    /// The package with `id`, or `None` for an id from another workspace.
    #[must_use]
    pub fn package(&self, id: PackageId) -> Option<&Package> {
        match id.index() {
            0 => Some(&self.root_package),
            index => self.packages.get(index - 1),
        }
    }

    /// Looks a package up by its `name`.
    #[must_use]
    pub fn package_by_name(&self, name: &str) -> Option<&Package> {
        self.all_packages()
            .find(|package| package.name.as_deref() == Some(name))
    }

    /// Whether `[workspace] ignore-packages` excludes the package with `id`.
    #[must_use]
    pub fn is_ignored(&self, id: PackageId) -> bool {
        self.ignored.get(id.index()).copied().unwrap_or(false)
    }
}
