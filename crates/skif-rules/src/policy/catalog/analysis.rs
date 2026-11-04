//! Decides, per dependency, how it relates to the workspace catalogs.
//!
//! One pass over the workspace yields a [`Finding`] for every instance that a catalog rule reports; the rules pick
//! the findings of their own kind.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Map, Value};
use skif_config::{Catalog, CatalogMode, Selector};
use skif_core::{DependencyKind, Fix, FixOp, FixSafety, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext};
use skif_specifier::Specifier;
use skif_workspace::{DependencyEntry, Package, PackageId, Workspace};

use super::super::specifiers::floor_version;
use super::names::{CatalogName, defined_catalogs, defining, referenced};
use super::support::{active_packages, set_specifier, value_location};
use super::target::{CatalogFile, add_entry, creatable_file, create_default_catalog, existing_file};

/// A dependency entry together with the package that declares it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Instance<'w> {
    pub package: &'w Package,
    pub entry: &'w DependencyEntry,
}

/// What a catalog rule reports about one instance.
#[derive(Debug, Clone)]
pub(super) enum FindingKind {
    /// `NotUsingCatalog`: the dependency has exactly one catalog entry and the instance does not use it.
    NotUsingCatalog { catalog: CatalogName },
    /// `NotUsingCatalogAndCatalogUnknown`: several catalogs are candidates, skif does not choose.
    NotUsingCatalogAndCatalogUnknown,
    /// `DiffersToCatalog`: a sibling uses the catalog (`None` when siblings use different catalogs).
    DiffersToCatalog { catalog: Option<CatalogName> },
    /// `MissingFromCatalog`: no entry yet; `catalog_ops` add it.
    MissingFromCatalog {
        catalog: CatalogName,
        catalog_ops: Vec<FixOp>,
    },
    /// `MissingFromCatalogAndNonSemverMismatch`: the instances disagree and cannot be merged into one entry.
    MissingFromCatalogAndNonSemverMismatch,
    /// `CannotInferCatalogFile`: no file is known to hold the new entry.
    CannotInferCatalogFile,
}

impl FindingKind {
    /// The state name carried in `Diagnostic.detail`.
    pub(super) const fn state(&self) -> &'static str {
        match self {
            Self::NotUsingCatalog { .. } => "NotUsingCatalog",
            Self::NotUsingCatalogAndCatalogUnknown => "NotUsingCatalogAndCatalogUnknown",
            Self::DiffersToCatalog { .. } => "DiffersToCatalog",
            Self::MissingFromCatalog { .. } => "MissingFromCatalog",
            Self::MissingFromCatalogAndNonSemverMismatch => "MissingFromCatalogAndNonSemverMismatch",
            Self::CannotInferCatalogFile => "CannotInferCatalogFile",
        }
    }
}

#[derive(Debug)]
pub(super) struct Finding<'a> {
    pub instance: Instance<'a>,
    pub kind: &'a FindingKind,
}

/// A dependency entry by position: its package and its index in `Package::dependencies`.
type Slot = (PackageId, usize);

/// The instances of one external dependency, by position.
#[derive(Debug, Default)]
struct Usage {
    /// Instances with a concrete specifier, candidates for a catalog.
    plain: Vec<Slot>,
    /// Instances already written as `catalog:`.
    references: Vec<Slot>,
    packages: BTreeSet<PackageId>,
}

/// Whether a specifier could be replaced by `catalog:`. Workspace, link and file specifiers name something local
/// to a package and have no registry version to share.
pub(crate) fn is_catalogable(specifier: &Specifier) -> bool {
    !matches!(
        specifier,
        Specifier::Catalog(_)
            | Specifier::WorkspaceProtocol(_)
            | Specifier::Link(_)
            | Specifier::File(_)
            | Specifier::None
    )
}

/// Instances of every external dependency, grouped by name. Peer dependencies state a compatibility range, not a
/// version to install, so they are neither candidates nor counted.
///
/// It refers to entries by position rather than by reference, so one value can be computed once per run and shared by
/// the catalog rules and the policy provider (see [`LintContext::shared`]).
#[derive(Debug, Default)]
pub(crate) struct UsageIndex {
    by_name: BTreeMap<String, Usage>,
}

impl UsageIndex {
    /// Walks every active package once, skipping dependencies `settings.exclude` names and those that are
    /// workspace packages.
    pub(crate) fn build(workspace: &Workspace, settings: &Catalog) -> Self {
        let exclude = Selector::new(settings.exclude.iter().cloned()).unwrap_or_default();
        let mut by_name: BTreeMap<String, Usage> = BTreeMap::new();
        for package in active_packages(workspace) {
            for (position, entry) in package
                .dependencies
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.kind != DependencyKind::PeerDependencies)
                .filter(|(_, entry)| !exclude.matches(&entry.name, false))
                .filter(|(_, entry)| workspace.package_by_name(&entry.name).is_none())
            {
                if !by_name.contains_key(entry.name.as_str()) {
                    by_name.insert(entry.name.clone(), Usage::default());
                }
                let Some(usage) = by_name.get_mut(entry.name.as_str()) else {
                    continue;
                };
                usage.packages.insert(package.id);
                if entry.parsed.is_catalog() {
                    usage.references.push((package.id, position));
                } else if is_catalogable(&entry.parsed) {
                    usage.plain.push((package.id, position));
                }
            }
        }
        Self { by_name }
    }
}

/// The instances at `slots`, in order.
fn instances<'w>(workspace: &'w Workspace, slots: &[Slot]) -> Vec<Instance<'w>> {
    slots
        .iter()
        .filter_map(|&(id, position)| {
            let package = workspace.package(id)?;
            Some(Instance {
                package,
                entry: package.dependencies.get(position)?,
            })
        })
        .collect()
}

/// The dependencies the catalog rules own under `settings`. The policy rules leave the plain instances of these
/// dependencies to the catalog rules, which report them against the catalog, so exactly one rule owns each instance.
///
/// With `mode` other than `off` that is every dependency that has a catalog entry, is not excluded and is used in at
/// least `min-packages` packages. With `mode = "off"` it is every dependency a sibling already takes from a catalog
/// (`catalog-mismatch` reports the plain instances, see [`FindingSet::compute`]).
pub(crate) fn owned_dependencies<'i>(
    usages: &'i UsageIndex,
    workspace: &Workspace,
    settings: &Catalog,
) -> BTreeSet<&'i str> {
    if settings.mode == CatalogMode::Off {
        return usages
            .by_name
            .iter()
            .filter(|(name, usage)| {
                !usage.plain.is_empty()
                    && sibling_catalog(workspace, name, &instances(workspace, &usage.references)).is_some()
            })
            .map(|(name, _)| name.as_str())
            .collect();
    }
    usages
        .by_name
        .iter()
        .filter(|(name, usage)| {
            !usage.plain.is_empty()
                && usage.packages.len() >= settings.min_packages
                && !defining(&workspace.catalogs, name).is_empty()
        })
        .map(|(name, _)| name.as_str())
        .collect()
}

/// The version the new catalog entry gets: the shared specifier, or the highest one when all are semver.
fn entry_version(plain: &[Instance<'_>]) -> Option<String> {
    let first = plain.first()?;
    if plain
        .iter()
        .all(|instance| instance.entry.specifier == first.entry.specifier)
    {
        return Some(first.entry.specifier.clone());
    }
    if plain
        .iter()
        .any(|instance| instance.entry.parsed.get_node_version().is_none())
    {
        return None;
    }
    // Ranked as the version groups rank them, by the lowest version each specifier admits, so `^18` never outranks
    // `^18.3.1`.
    plain
        .iter()
        .max_by(|a, b| {
            floor_version(&a.entry.parsed)
                .cmp(&floor_version(&b.entry.parsed))
                .then_with(|| a.entry.parsed.cmp(&b.entry.parsed))
        })
        .map(|best| best.entry.specifier.clone())
}

enum Placement {
    /// Add the entry to the single existing catalog in this file.
    Add(CatalogFile),
    /// Create the default catalog in this file.
    Create(CatalogFile),
}

enum Decision {
    NotUsing(CatalogName),
    Ambiguous,
    Differs(Option<CatalogName>),
    Missing {
        catalog: CatalogName,
        version: String,
        placement: Placement,
    },
    Mixed,
    CannotInfer,
}

fn decide_missing(workspace: &Workspace, plain: &[Instance<'_>]) -> Decision {
    let Some(version) = entry_version(plain) else {
        return Decision::Mixed;
    };
    let catalogs = defined_catalogs(&workspace.catalogs);
    match catalogs.as_slice() {
        [] => creatable_file(workspace).map_or(Decision::CannotInfer, |file| Decision::Missing {
            catalog: CatalogName::Default,
            version,
            placement: Placement::Create(file),
        }),
        [only] => existing_file(workspace).map_or(Decision::CannotInfer, |file| Decision::Missing {
            catalog: only.clone(),
            version,
            placement: Placement::Add(file),
        }),
        _ => Decision::Ambiguous,
    }
}

fn decide_policy(workspace: &Workspace, name: &str, plain: &[Instance<'_>]) -> Decision {
    let mut defs = defining(&workspace.catalogs, name);
    match defs.len() {
        0 => decide_missing(workspace, plain),
        1 => defs.pop().map_or(Decision::Ambiguous, Decision::NotUsing),
        _ => Decision::Ambiguous,
    }
}

/// The catalog every resolvable `catalog:` reference to `name` selects. Outer `None`: no sibling uses a catalog;
/// inner `None`: siblings select different catalogs.
#[allow(
    clippy::option_option,
    reason = "the two absences mean different things to the caller"
)]
fn sibling_catalog(workspace: &Workspace, name: &str, references: &[Instance<'_>]) -> Option<Option<CatalogName>> {
    let catalogs: Vec<CatalogName> = references
        .iter()
        .filter(|instance| workspace.catalogs.resolve(name, &instance.entry.specifier).is_some())
        .filter_map(|instance| referenced(&instance.entry.specifier))
        .collect();
    let first = catalogs.first()?;
    Some(catalogs.iter().all(|catalog| catalog == first).then(|| first.clone()))
}

fn finding_kind(workspace: &Workspace, name: &str, decision: &Decision, creations: &Map<String, Value>) -> FindingKind {
    match decision {
        Decision::NotUsing(catalog) => FindingKind::NotUsingCatalog {
            catalog: catalog.clone(),
        },
        Decision::Ambiguous => FindingKind::NotUsingCatalogAndCatalogUnknown,
        Decision::Differs(catalog) => FindingKind::DiffersToCatalog {
            catalog: catalog.clone(),
        },
        Decision::Missing {
            catalog,
            version,
            placement,
        } => FindingKind::MissingFromCatalog {
            catalog: catalog.clone(),
            catalog_ops: vec![match placement {
                Placement::Add(file) => add_entry(workspace, file, catalog, name, version),
                Placement::Create(file) => create_default_catalog(file, creations.clone()),
            }],
        },
        Decision::Mixed => FindingKind::MissingFromCatalogAndNonSemverMismatch,
        Decision::CannotInfer => FindingKind::CannotInferCatalogFile,
    }
}

/// Every instance the catalog rules report, by position, with what is reported about it.
///
/// Several instances of a dependency share one entry of `kinds`. The set holds no borrows, so it is computed once per
/// run and shared by the rules that read it (see [`findings`]).
#[derive(Debug, Default)]
pub(super) struct FindingSet {
    kinds: Vec<FindingKind>,
    items: Vec<(Slot, usize)>,
}

impl FindingSet {
    /// The findings in dependency-name order, each with its instance resolved against `workspace`.
    pub(super) fn iter<'a>(&'a self, workspace: &'a Workspace) -> impl Iterator<Item = Finding<'a>> + 'a {
        self.items.iter().filter_map(move |&((id, position), kind)| {
            let package = workspace.package(id)?;
            Some(Finding {
                instance: Instance {
                    package,
                    entry: package.dependencies.get(position)?,
                },
                kind: self.kinds.get(kind)?,
            })
        })
    }

    fn compute(workspace: &Workspace, settings: &Catalog, usages: &UsageIndex) -> Self {
        let mut decided: Vec<(&str, &Usage, Decision)> = Vec::new();
        for (name, usage) in &usages.by_name {
            if usage.plain.is_empty() {
                continue;
            }
            let decision = match settings.mode {
                CatalogMode::Off => match sibling_catalog(workspace, name, &instances(workspace, &usage.references)) {
                    Some(catalog) => Decision::Differs(catalog),
                    None => continue,
                },
                _ if usage.packages.len() >= settings.min_packages => {
                    decide_policy(workspace, name, &instances(workspace, &usage.plain))
                }
                _ => continue,
            };
            decided.push((name.as_str(), usage, decision));
        }
        let creations: Map<String, Value> = decided
            .iter()
            .filter_map(|(name, _, decision)| match decision {
                Decision::Missing {
                    version,
                    placement: Placement::Create(_),
                    ..
                } => Some(((*name).to_owned(), Value::String(version.clone()))),
                _ => None,
            })
            .collect();
        let mut set = Self::default();
        for (name, usage, decision) in decided {
            let at = set.kinds.len();
            set.kinds.push(finding_kind(workspace, name, &decision, &creations));
            set.items.extend(usage.plain.iter().map(|slot| (*slot, at)));
        }
        set
    }
}

/// The usages of external dependencies in the workspace, computed once per run.
pub(crate) fn usages(cx: &LintContext<'_>) -> Arc<UsageIndex> {
    cx.shared(|| UsageIndex::build(cx.workspace, &cx.config.catalog))
}

/// Every instance the catalog rules report, grouped by dependency name, computed once per run however many rules
/// read it.
///
/// With `mode = "off"` only instances that differ from a catalog their siblings use are reported. Otherwise a
/// dependency is owned by the catalog policy once it is declared in at least `min-packages` packages.
pub(super) fn findings(cx: &LintContext<'_>) -> Arc<FindingSet> {
    cx.shared(|| FindingSet::compute(cx.workspace, &cx.config.catalog, &usages(cx)))
}

/// The operations rewriting `instance` to `catalog`, after `catalog_ops`.
pub(super) fn use_catalog_ops(instance: Instance<'_>, catalog: &CatalogName, catalog_ops: &[FixOp]) -> Vec<FixOp> {
    let mut ops = catalog_ops.to_vec();
    ops.push(set_specifier(instance.package, instance.entry, &catalog.reference()));
    ops
}

/// The severity the rules that police catalog usage propose: an error under `[catalog] mode = "enforce"`, where a
/// dependency that should come from a catalog is a violation. Otherwise they keep their own default, and a level
/// configured for the rule always wins (see [`skif_core::Diagnostic::default_severity`]).
pub(super) fn enforce_severity(cx: &LintContext<'_>) -> Option<Severity> {
    (cx.config.catalog.mode == CatalogMode::Enforce).then_some(Severity::Error)
}

/// Reports `finding` under `meta` with the shared package, dependency, state and location fields, proposing
/// `severity` when it is given.
pub(super) fn emit(
    out: &mut Emitter<'_>,
    meta: &'static RuleMeta,
    severity: Option<Severity>,
    finding: &Finding<'_>,
    message: String,
    help: Option<String>,
    fix: Option<Fix>,
) {
    let Instance { package, entry } = finding.instance;
    let mut builder = diagnostic(meta, message, value_location(package, entry))
        .package(package.display_name())
        .package_id(package.id)
        .dependency(entry.name.clone())
        .specifier(entry.specifier.clone())
        .detail(finding.kind.state());
    if let Some(severity) = severity {
        builder = builder.default_severity(severity);
    }
    if let Some(help) = help {
        builder = builder.help(help);
    }
    if let Some(fix) = fix {
        builder = builder.fix(fix);
    }
    out.emit(builder.build());
}

/// An unsafe fix that rewrites `instance` to `catalog`, after `catalog_ops`.
pub(super) fn use_catalog_fix(instance: Instance<'_>, catalog: &CatalogName, catalog_ops: &[FixOp]) -> Fix {
    Fix::new(
        format!("Use `{}` for `{}`", catalog.reference(), instance.entry.name),
        FixSafety::Unsafe,
        use_catalog_ops(instance, catalog, catalog_ops),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, workspace};

    fn entry_versions(specifiers: &[&str]) -> Option<String> {
        let files: Vec<(String, String)> = specifiers
            .iter()
            .enumerate()
            .map(|(index, specifier)| {
                (
                    format!("packages/p{index}/package.json"),
                    format!(r#"{{"name":"p{index}","version":"1.0.0","dependencies":{{"react":"{specifier}"}}}}"#),
                )
            })
            .collect();
        let mut borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        borrowed.push((
            "package.json",
            r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]}"#,
        ));
        let workspace = workspace(&borrowed, &config(""));
        let plain: Vec<Instance<'_>> = workspace
            .all_packages()
            .flat_map(|package| {
                package
                    .dependencies
                    .iter()
                    .filter(|entry| entry.name == "react")
                    .map(move |entry| Instance { package, entry })
            })
            .collect();
        entry_version(&plain)
    }

    #[test]
    fn the_entry_takes_the_most_specific_highest_version_not_the_vaguest() {
        assert_eq!(entry_versions(&["^18", "^18.3.1"]).as_deref(), Some("^18.3.1"));
        assert_eq!(entry_versions(&["^18.3.1", "^18"]).as_deref(), Some("^18.3.1"));
        assert_eq!(
            entry_versions(&["^18.2.0", "^18.3.1", "^17.0.0"]).as_deref(),
            Some("^18.3.1")
        );
        assert_eq!(entry_versions(&["^18", "^18"]).as_deref(), Some("^18"));
    }
}
