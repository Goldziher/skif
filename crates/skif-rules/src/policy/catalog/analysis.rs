//! Decides, per dependency, how it relates to the workspace catalogs.
//!
//! One pass over the workspace yields a [`Finding`] for every instance that a catalog rule reports; the rules pick
//! the findings of their own kind.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use skif_config::{CatalogMode, Selector};
use skif_core::{DependencyKind, Fix, FixOp, FixSafety, RuleMeta, diagnostic};
use skif_lint::{Emitter, LintContext};
use skif_specifier::Specifier;
use skif_workspace::{DependencyEntry, Package, PackageId, Workspace};

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
pub(super) struct Finding<'w> {
    pub instance: Instance<'w>,
    pub kind: FindingKind,
}

#[derive(Default)]
struct Usage<'w> {
    /// Instances with a concrete specifier, candidates for a catalog.
    plain: Vec<Instance<'w>>,
    /// Instances already written as `catalog:`.
    references: Vec<Instance<'w>>,
    packages: BTreeSet<PackageId>,
}

/// Whether a specifier could be replaced by `catalog:`. Workspace, link and file specifiers name something local
/// to a package and have no registry version to share.
fn is_catalogable(specifier: &Specifier) -> bool {
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
fn usages<'w>(workspace: &'w Workspace, exclude: &Selector) -> BTreeMap<&'w str, Usage<'w>> {
    let mut usages: BTreeMap<&str, Usage<'_>> = BTreeMap::new();
    for package in active_packages(workspace) {
        for entry in package
            .dependencies
            .iter()
            .filter(|entry| entry.kind != DependencyKind::PeerDependencies)
            .filter(|entry| !exclude.matches(&entry.name, false))
            .filter(|entry| workspace.package_by_name(&entry.name).is_none())
        {
            let usage = usages.entry(entry.name.as_str()).or_default();
            usage.packages.insert(package.id);
            let instance = Instance { package, entry };
            if entry.parsed.is_catalog() {
                usage.references.push(instance);
            } else if is_catalogable(&entry.parsed) {
                usage.plain.push(instance);
            }
        }
    }
    usages
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
    plain
        .iter()
        .max_by(|a, b| a.entry.parsed.cmp(&b.entry.parsed))
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

/// Every instance the catalog rules report, grouped by dependency name.
///
/// With `mode = "off"` only instances that differ from a catalog their siblings use are reported. Otherwise a
/// dependency is owned by the catalog policy once it is declared in at least `min-packages` packages.
pub(super) fn findings<'w>(cx: &LintContext<'w>) -> Vec<Finding<'w>> {
    let workspace = cx.workspace;
    let settings = &cx.config.catalog;
    let exclude = Selector::new(settings.exclude.iter().cloned()).unwrap_or_default();
    let mut decided: Vec<(&str, Vec<Instance<'w>>, Decision)> = Vec::new();
    for (name, usage) in usages(workspace, &exclude) {
        if usage.plain.is_empty() {
            continue;
        }
        let decision = match settings.mode {
            CatalogMode::Off => match sibling_catalog(workspace, name, &usage.references) {
                Some(catalog) => Decision::Differs(catalog),
                None => continue,
            },
            _ if usage.packages.len() >= settings.min_packages => decide_policy(workspace, name, &usage.plain),
            _ => continue,
        };
        decided.push((name, usage.plain, decision));
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
    decided
        .into_iter()
        .flat_map(|(name, plain, decision)| {
            let kind = finding_kind(workspace, name, &decision, &creations);
            plain.into_iter().map(move |instance| Finding {
                instance,
                kind: kind.clone(),
            })
        })
        .collect()
}

/// The operations rewriting `instance` to `catalog`, after `catalog_ops`.
pub(super) fn use_catalog_ops(instance: Instance<'_>, catalog: &CatalogName, catalog_ops: &[FixOp]) -> Vec<FixOp> {
    let mut ops = catalog_ops.to_vec();
    ops.push(set_specifier(instance.package, instance.entry, &catalog.reference()));
    ops
}

/// Reports `finding` under `meta` with the shared package, dependency, state and location fields.
pub(super) fn emit(
    out: &mut Emitter<'_>,
    meta: &'static RuleMeta,
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
        .detail(finding.kind.state());
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
    Fix {
        title: format!("Use `{}` for `{}`", catalog.reference(), instance.entry.name),
        safety: FixSafety::Unsafe,
        ops: use_catalog_ops(instance, catalog, catalog_ops),
    }
}
