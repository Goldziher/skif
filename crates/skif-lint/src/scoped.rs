use std::collections::HashMap;
use std::path::PathBuf;

use skif_config::{Config, Level, Override};
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
    overrides: Vec<Override>,
    /// Whether the override at the same index sets rule levels, a cap or suppressions.
    relevant: Vec<bool>,
    max_level: Option<Level>,
    cli_levels: HashMap<String, Level>,
}

#[derive(Debug)]
struct PackageFacts {
    name: String,
    /// The absolute directory.
    dir: PathBuf,
    /// The workspace-relative directory; empty for the root.
    relative: PathBuf,
}

impl Scopes {
    pub fn new(workspace: &Workspace, config: &Config) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let packages = workspace
            .iter()
            .enumerate()
            .map(|(index, (_, package))| {
                let name = package.display_name().to_owned();
                by_name.entry(name.clone()).or_default().push(index);
                PackageFacts {
                    name,
                    dir: if package.dir.as_os_str().is_empty() {
                        workspace.root.clone()
                    } else {
                        workspace.root.join(&package.dir)
                    },
                    relative: package.dir.clone(),
                }
            })
            .collect();
        let relevant = config
            .overrides
            .iter()
            .map(|scoped| scoped.max_level.is_some() || !scoped.rules.is_empty() || !scoped.ignore.is_empty())
            .collect();
        Self {
            root: workspace.root.clone(),
            packages,
            by_name,
            overrides: config.overrides.clone(),
            relevant,
            max_level: config.lint.max_level,
            cli_levels: config
                .cli_levels
                .iter()
                .map(|(id, level)| (id.clone(), *level))
                .collect(),
        }
    }

    /// The package a diagnostic is about: by id, else by a unique name, else the member whose directory holds the
    /// diagnostic's file.
    fn package_of(&self, diagnostic: &Diagnostic) -> Option<&PackageFacts> {
        if let Some(id) = diagnostic.package_id {
            return self.packages.get(id.index());
        }
        let named = diagnostic.package.as_deref().and_then(|name| self.by_name.get(name));
        if let Some([only]) = named.map(Vec::as_slice) {
            return self.packages.get(*only);
        }
        self.packages
            .iter()
            .filter(|package| {
                !package.relative.as_os_str().is_empty() && diagnostic.location.path.starts_with(&package.relative)
            })
            .max_by_key(|package| package.relative.components().count())
    }

    /// The indices of the overrides that cover `diagnostic` and set levels, a cap or suppressions, in
    /// configuration order.
    pub fn applicable(&self, diagnostic: &Diagnostic) -> Vec<usize> {
        if !self.relevant.iter().any(|relevant| *relevant) {
            return Vec::new();
        }
        let package = self.package_of(diagnostic);
        let package = package.map(|facts| (facts.name.as_str(), facts.dir.as_path()));
        let file = self.root.join(&diagnostic.location.path);
        self.overrides
            .iter()
            .enumerate()
            .filter(|(index, scoped)| self.relevant[*index] && scoped.applies_to(package, Some(file.as_path())))
            .map(|(index, _)| index)
            .collect()
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
