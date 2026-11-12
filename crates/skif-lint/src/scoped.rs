use skif_core::HashMap;
use std::borrow::Cow;
use std::path::PathBuf;

use skif_config::{Config, Level, Override, Selector};
use skif_core::{Diagnostic, Severity};
use skif_workspace::Workspace;

use crate::context::EnabledRules;

/// `[[overrides]]` and `[lint] max-level`, resolved against a workspace: which overrides cover a diagnostic and the
/// severity it ends up with.
///
/// The severity of a diagnostic is the base level of its rule, then the level of each override that covers it in
/// configuration order, then the level given on the command line, and finally the caps: the global `max-level` and
/// the `max-level` of each covering override.
#[derive(Debug)]
pub struct Scopes {
    root: PathBuf,
    packages: Vec<PackageFacts>,
    by_name: HashMap<String, Vec<usize>>,
    /// The package at each non-root workspace-relative directory.
    by_dir: HashMap<PathBuf, usize>,
    overrides: Vec<Override>,
    /// Whether any override sets rule levels, a cap or suppressions.
    any_relevant: bool,
    /// Per package, the relevant overrides without a file selector that cover it, in configuration order: resolved
    /// once here, so a diagnostic costs a lookup instead of a match against every override.
    covering: Vec<Vec<usize>>,
    /// The same for a diagnostic that names no package.
    covering_unowned: Vec<usize>,
    /// The relevant overrides with a file selector, which depend on the diagnostic's file and are checked per
    /// diagnostic.
    file_scoped: Vec<usize>,
    max_level: Option<Level>,
    cli_levels: HashMap<String, Level>,
}

/// For each of `packages`, the overrides of `package_scoped` (indices into `overrides`, ascending) that cover it, in
/// configuration order.
///
/// An override that selects by literal names and directories only (every package file's `packages = ["./"]`) is
/// found through an index of those literals, so the packages are not each matched against every override.
fn covering_overrides(packages: &[PackageFacts], overrides: &[Override], package_scoped: &[usize]) -> Vec<Vec<usize>> {
    let mut by_literal: HashMap<&str, Vec<usize>> = HashMap::default();
    let mut matched = Vec::new();
    for &index in package_scoped {
        match overrides[index].literal_packages() {
            Some(literals) => {
                for literal in literals {
                    by_literal.entry(literal).or_default().push(index);
                }
            }
            None => matched.push(index),
        }
    }
    packages
        .iter()
        .map(|facts| {
            let package = Some((facts.name.as_str(), facts.dir.as_path()));
            let dir = skif_core::slash(&facts.dir);
            let mut covering: Vec<usize> = matched
                .iter()
                .copied()
                .filter(|index| overrides[*index].applies_to(package, None))
                .collect();
            for key in [facts.name.as_str(), dir.as_str()] {
                if let Some(indices) = by_literal.get(Selector::literal_key(key).as_ref()) {
                    covering.extend_from_slice(indices);
                }
            }
            covering.sort_unstable();
            covering.dedup();
            covering
        })
        .collect()
}

#[derive(Debug)]
struct PackageFacts {
    name: String,
    /// The absolute directory.
    dir: PathBuf,
}

impl Scopes {
    pub fn new(workspace: &Workspace, config: &Config) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::default();
        let mut by_dir: HashMap<PathBuf, usize> = HashMap::default();
        let packages: Vec<PackageFacts> = workspace
            .iter()
            .enumerate()
            .map(|(index, (_, package))| {
                let name = package.display_name().to_owned();
                by_name.entry(name.clone()).or_default().push(index);
                if !package.dir.as_os_str().is_empty() {
                    by_dir.insert(package.dir.clone(), index);
                }
                PackageFacts {
                    name,
                    dir: if package.dir.as_os_str().is_empty() {
                        workspace.root.clone()
                    } else {
                        workspace.root.join(&package.dir)
                    },
                }
            })
            .collect();
        let relevant: Vec<bool> = config
            .overrides
            .iter()
            .map(|scoped| scoped.max_level.is_some() || !scoped.rules.is_empty() || !scoped.ignore.is_empty())
            .collect();
        let (file_scoped, package_scoped): (Vec<usize>, Vec<usize>) = (0..config.overrides.len())
            .filter(|index| relevant[*index])
            .partition(|index| !config.overrides[*index].files.is_empty());
        let covering = covering_overrides(&packages, &config.overrides, &package_scoped);
        let covering_unowned: Vec<usize> = package_scoped
            .iter()
            .copied()
            .filter(|index| config.overrides[*index].applies_to(None, None))
            .collect();
        Self {
            root: workspace.root.clone(),
            packages,
            by_name,
            by_dir,
            overrides: config.overrides.clone(),
            any_relevant: relevant.contains(&true),
            covering,
            covering_unowned,
            file_scoped,
            max_level: config.lint.max_level,
            cli_levels: config
                .cli_levels
                .iter()
                .map(|(id, level)| (id.clone(), *level))
                .collect(),
        }
    }

    /// The index of the package a diagnostic is about: by id, else by a unique name, else the member whose
    /// directory holds the diagnostic's file.
    fn package_of(&self, diagnostic: &Diagnostic) -> Option<usize> {
        if let Some(id) = diagnostic.package_id {
            return (id.index() < self.packages.len()).then_some(id.index());
        }
        let named = diagnostic.package.as_deref().and_then(|name| self.by_name.get(name));
        if let Some([only]) = named.map(Vec::as_slice) {
            return Some(*only);
        }
        diagnostic
            .location
            .path
            .ancestors()
            .filter(|dir| !dir.as_os_str().is_empty())
            .find_map(|dir| self.by_dir.get(dir))
            .copied()
    }

    /// The indices of the overrides that cover `diagnostic` and set levels, a cap or suppressions, in
    /// configuration order. Borrowed unless an override selects by file.
    pub fn applicable(&self, diagnostic: &Diagnostic) -> Cow<'_, [usize]> {
        if !self.any_relevant {
            return Cow::Borrowed(<&[usize]>::default());
        }
        let package = self.package_of(diagnostic);
        let package_scoped = package.map_or(self.covering_unowned.as_slice(), |index| {
            self.covering[index].as_slice()
        });
        if self.file_scoped.is_empty() {
            return Cow::Borrowed(package_scoped);
        }
        let facts = package.map(|index| (self.packages[index].name.as_str(), self.packages[index].dir.as_path()));
        let file = self.root.join(&diagnostic.location.path);
        let mut covering: Vec<usize> = package_scoped
            .iter()
            .copied()
            .chain(
                self.file_scoped
                    .iter()
                    .copied()
                    .filter(|index| self.overrides[*index].applies_to(facts, Some(file.as_path()))),
            )
            .collect();
        covering.sort_unstable();
        Cow::Owned(covering)
    }

    /// The severity `diagnostic` is reported with, or `None` when it is off where it is located.
    pub fn severity(&self, enabled: &EnabledRules, diagnostic: &Diagnostic, applicable: &[usize]) -> Option<Severity> {
        if self.overrides.is_empty() && self.max_level.is_none() {
            return enabled.effective(diagnostic);
        }
        let id = diagnostic.rule.as_str();
        let (mut level, mut configured) = enabled.base(id)?;
        for index in applicable {
            let set = self.overrides[*index].rules.get(id).and_then(|rule| rule.severity);
            if let Some(set) = set {
                level = set.severity();
                configured = true;
            }
        }
        if let Some(set) = self.cli_levels.get(id) {
            level = set.severity();
            configured = true;
        }
        let mut severity = level?;
        if !configured {
            severity = diagnostic.default_severity.unwrap_or(severity);
        }
        let caps = self
            .max_level
            .into_iter()
            .chain(applicable.iter().filter_map(|index| self.overrides[*index].max_level));
        for cap in caps {
            severity = severity.min(cap.severity()?);
        }
        Some(severity)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::{Path, PathBuf};

    use skif_config::{Ceiling, ConfigCache, ConfigLoader};
    use skif_core::MemoryFs;

    use super::{PackageFacts, covering_overrides};

    #[test]
    fn literal_and_glob_overrides_cover_packages_once_each_in_configuration_order() {
        let fs = MemoryFs::new().with_file(
            "/ws/skif.toml",
            "[[overrides]]\npackages = [\"a\", \"./packages/a\"]\n[overrides.rules]\ndemo = \"error\"\n\
             [[overrides]]\npackages = [\"*\"]\n[overrides.rules]\ndemo = \"info\"\n\
             [[overrides]]\npackages = [\"./packages/b\"]\n[overrides.rules]\ndemo = \"error\"\n\
             [[overrides]]\npackages = [\"b\", \"!root\"]\n[overrides.rules]\ndemo = \"off\"\n",
        );
        let config = ConfigLoader::new(&fs, &ConfigCache::new())
            .resolve(Path::new("/ws"), &Ceiling::RootOnly)
            .unwrap()
            .config;
        let facts = |name: &str, dir: &str| PackageFacts {
            name: name.to_owned(),
            dir: PathBuf::from(dir),
        };
        let packages = [
            facts("root", "/ws"),
            facts("a", "/ws/packages/a"),
            facts("b", "/ws/packages/b"),
            facts("c", "/ws/packages/c"),
        ];
        assert!(config.overrides[0].literal_packages().is_some());
        assert!(config.overrides[1].literal_packages().is_none());
        assert_eq!(
            covering_overrides(&packages, &config.overrides, &[0, 1, 2, 3]),
            [vec![1], vec![0, 1], vec![1, 2, 3], vec![1]]
        );
    }
}
