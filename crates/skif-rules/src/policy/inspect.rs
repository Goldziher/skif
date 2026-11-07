//! The state of every dependency instance the policies govern, valid or not: what `skif list` and `skif json`
//! show.
//!
//! The listing reuses the analysis behind the policy rules, so the version group and the state of an instance are
//! the ones a lint run decides. An instance that a lint run reported carries that diagnostic's state and severity;
//! that covers the catalog rules, whose states are not part of the version-group analysis.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::Serialize;
use skif_config::{Config, Prefer};
use skif_core::{Diagnostic, Location, Severity};
use skif_specifier::Specifier;
use skif_workspace::Workspace;

use super::analysis::{Analysis, GroupSummary};
use super::catalog;
use super::groups::Policy;
use super::instances::{Instance, InstanceId};

/// syncpack's classes of instance states, as `statusType` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[non_exhaustive]
pub enum StatusType {
    /// Nothing to report.
    Valid,
    /// Wrong, with an automatic fix.
    Fixable,
    /// Wrong, without an automatic fix.
    Unfixable,
    /// Two policies require incompatible outcomes.
    Conflict,
    /// The configuration or the workspace is probably wrong.
    Suspect,
}

impl StatusType {
    /// The class of a state name (syncpack's, or skif's own `UnusedGroup`, `InvalidPin` and
    /// `DiffersToOtherCatalog`).
    #[must_use]
    pub fn of(code: &str) -> Self {
        match code {
            "IsCatalog"
            | "IsCatalogDefinition"
            | "IsIgnored"
            | "IsLocalAndValid"
            | "IsIdenticalToLocal"
            | "SatisfiesLocal"
            | "IsHighestOrLowestSemver"
            | "SatisfiesHighestOrLowestSemver"
            | "IsNonSemverButIdentical"
            | "IsIdenticalToPin"
            | "SatisfiesSameRangeGroup"
            | "MatchesSemverGroup"
            | "SatisfiesSameMinorGroup"
            | "IsIdenticalToSnapTarget"
            | "SatisfiesSnapTarget"
            | "IsIdenticalToCatalog" => Self::Valid,
            "IsBanned"
            | "DiffersToCatalog"
            | "NotUsingCatalog"
            | "MissingFromCatalog"
            | "DiffersToLocal"
            | "DiffersToHighestOrLowestSemver"
            | "DiffersToHighestOrLowestSemverMinor"
            | "DiffersToNpmRegistry"
            | "DiffersToSnapTarget"
            | "DiffersToPin"
            | "SemverRangeMismatch"
            | "PinOverridesSemverRange"
            | "PinOverridesSemverRangeMismatch"
            | "SameMinorOverridesSemverRange"
            | "SameMinorOverridesSemverRangeMismatch" => Self::Fixable,
            "MatchConflictsWithHighestOrLowestSemver"
            | "MismatchConflictsWithHighestOrLowestSemver"
            | "MatchConflictsWithSnapTarget"
            | "MismatchConflictsWithSnapTarget"
            | "MatchConflictsWithLocal"
            | "MismatchConflictsWithLocal" => Self::Conflict,
            "RefuseToBanLocal"
            | "RefuseToPinLocal"
            | "RefuseToSnapLocal"
            | "InvalidLocalVersion"
            | "DependsOnMissingSnapTarget"
            | "DependsOnMissingCatalogDefinition"
            | "RefuseToCatalogLocal"
            | "UnusedGroup"
            | "InvalidPin" => Self::Suspect,
            _ => Self::Unfixable,
        }
    }

    /// The name, as `statusType` carries it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Valid => "Valid",
            Self::Fixable => "Fixable",
            Self::Unfixable => "Unfixable",
            Self::Conflict => "Conflict",
            Self::Suspect => "Suspect",
        }
    }
}

/// The version group that owns an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct GroupRef {
    /// Position in `[[version-groups]]`; `None` for the catch-all group and for the catalog.
    pub index: Option<usize>,
    /// The group's `label`.
    pub label: Option<String>,
    /// syncpack's name of the policy: `HighestSemver`, `LowestSemver`, `Pinned`, `Banned`, `Ignored`, `SameRange`,
    /// `SameMinor`, `SnappedTo`, `SemverRangeOnly`, or `Catalog` for an instance the catalog rules own.
    pub variant: &'static str,
}

/// One dependency instance and its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct InstanceStatus {
    /// The dependency name; for a package's own version, the package name.
    pub dependency: String,
    /// syncpack's name of the dependency type: `prod`, `dev`, `peer`, `optional`, `local`, `resolutions`,
    /// `overrides`, `pnpm-overrides` or a custom type's name.
    pub dependency_type: String,
    /// The manifest field the version is declared in (`dependencies`, `version`, `engines.node`, ...).
    pub property: String,
    /// The display name of the package that declares the instance.
    pub package: String,
    /// Where the version is declared, relative to the workspace root.
    pub location: Location,
    /// The specifier as written.
    pub specifier: String,
    /// The specifier's type, as `specifier-types` selectors name it (`exact`, `range`, `workspace-protocol`, ...).
    pub specifier_type: &'static str,
    /// The owning version group.
    pub group: GroupRef,
    /// The range the instance's semver group asks for.
    pub preferred_range: Option<String>,
    /// The state name: syncpack's, or one of skif's own (see [`StatusType::of`]).
    pub status_code: String,
    /// The class of `status_code`.
    pub status_type: StatusType,
    /// The rule a lint run reports the instance under, when it does.
    pub rule: Option<String>,
    /// The severity the lint run reported it at; `None` when it was not reported (valid, or the rule is off or
    /// ignored for it).
    pub severity: Option<Severity>,
    /// The specifier the instance should have, when it is known.
    pub expected: Option<String>,
    /// The type of `expected`, named as `specifier_type` names it.
    pub expected_type: Option<&'static str>,
    /// Whether the dependency is a package of the workspace.
    pub is_local: bool,
}

/// The state of every instance the version groups or the catalog rules govern, ordered by version group (configured
/// groups in order, the catch-all, then the catalog), then dependency name, then where it is declared.
///
/// `enabled` says whether a rule is on, which decides, as in a lint run, whether the catalog rules own a dependency.
/// `diagnostics` are the findings of a lint run of the same workspace and configuration: a diagnostic of a policy or
/// catalog rule located on an instance gives it that state, rule and severity. Instances outside the scope of every
/// group (`[policy] dependency-types`) are not listed.
#[must_use]
pub fn inspect(
    workspace: &Workspace,
    config: &Config,
    enabled: &dyn Fn(&str) -> bool,
    diagnostics: &[Diagnostic],
) -> Vec<InstanceStatus> {
    let catalog_owns = enabled(catalog::owning_rule(config.catalog.mode));
    let usages = catalog::UsageIndex::build(workspace, config);
    let catalog_deps = if catalog_owns {
        catalog::owned_dependencies(&usages, workspace, &config.catalog)
    } else {
        BTreeSet::new()
    };
    let analysis = Analysis::run_with(workspace, config, &catalog_deps);
    let rules = policy_rules();
    let reported: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.detail.is_some() && rules.contains(diagnostic.rule.as_str()))
        .collect();
    let catalog_index = analysis.groups.len();
    let mut rows: Vec<(usize, InstanceId, InstanceStatus)> = Vec::new();
    for (id, instance) in analysis.instances.iter() {
        // The catch-all leaves `catalog:` references to the catalog rules, as it leaves the dependencies they own.
        let owner = match analysis.owners[id.index()] {
            Some(owner) if !(instance.specifier.is_catalog() && owner + 1 == catalog_index) => owner,
            Some(_) => catalog_index,
            None if instance.catalog_owned || instance.specifier.is_catalog() => catalog_index,
            None => continue,
        };
        let group = analysis.groups.get(owner);
        let mut status = base_status(&analysis, id, instance, group);
        if let Some(diagnostic) = finding_on(&reported, instance) {
            let code = diagnostic.detail.clone().unwrap_or_default();
            if code != status.status_code {
                // The analysis's expectation belongs to its own state, not to the catalog rule's.
                status.expected = None;
                status.expected_type = None;
            }
            status.status_type = StatusType::of(&code);
            status.status_code = code;
            status.rule = Some(diagnostic.rule.as_str().to_owned());
            status.severity = Some(diagnostic.severity);
        }
        rows.push((owner, id, status));
    }
    rows.sort_by(|a, b| (a.0, &a.2.dependency, a.1).cmp(&(b.0, &b.2.dependency, b.1)));
    rows.into_iter().map(|(_, _, status)| status).collect()
}

/// The ids of the rules whose diagnostics carry an instance state.
fn policy_rules() -> BTreeSet<&'static str> {
    let mut ids: BTreeSet<&'static str> = super::rules().iter().map(|meta| meta.id).collect();
    for rule in catalog::rules() {
        ids.insert(rule.meta().id);
    }
    ids
}

/// The most severe diagnostic located on `instance`: same file, same dependency, inside the instance's span.
fn finding_on<'d>(reported: &[&'d Diagnostic], instance: &Instance<'_>) -> Option<&'d Diagnostic> {
    let location = instance.location();
    reported
        .iter()
        .copied()
        .filter(|diagnostic| diagnostic.dependency.as_deref() == Some(instance.name.as_str()))
        .filter(|diagnostic| within(&diagnostic.location, &location))
        .min_by_key(|diagnostic| std::cmp::Reverse(diagnostic.severity))
}

fn within(inner: &Location, outer: &Location) -> bool {
    inner.path == outer.path
        && match (inner.span, outer.span) {
            (Some(inner), Some(outer)) => inner.start >= outer.start && inner.end <= outer.end,
            (None, None) => true,
            _ => false,
        }
}

fn base_status(
    analysis: &Analysis<'_>,
    id: InstanceId,
    instance: &Instance<'_>,
    group: Option<&GroupSummary>,
) -> InstanceStatus {
    let (code, expected) = match &analysis.verdicts[id.index()] {
        Some(verdict) => (
            verdict.state.name().to_owned(),
            verdict.target.as_ref().map(|target| target.get_raw().to_owned()),
        ),
        None => (
            valid_code(analysis, id, instance, group).to_owned(),
            Some(instance.raw().to_owned()),
        ),
    };
    InstanceStatus {
        dependency: instance.name.clone(),
        dependency_type: instance.ty().token().to_owned(),
        property: instance.ty().label().to_owned(),
        package: instance.package.display_name().to_owned(),
        location: instance.location(),
        specifier: instance.raw().to_owned(),
        specifier_type: instance.specifier.get_config_identifier(),
        group: group.map_or(
            GroupRef {
                index: None,
                label: None,
                variant: "Catalog",
            },
            |group| GroupRef {
                index: group.configured,
                label: group.label.clone(),
                variant: variant(&group.policy),
            },
        ),
        preferred_range: analysis.preferred[id.index()].clone(),
        status_type: StatusType::of(&code),
        status_code: code,
        rule: None,
        severity: None,
        expected_type: expected
            .as_deref()
            .map(|raw| Specifier::new(raw).get_config_identifier()),
        expected,
        is_local: instance.is_local,
    }
}

/// syncpack's `variant_label` of a version group.
const fn variant(policy: &Policy) -> &'static str {
    match policy {
        Policy::Pinned(_) | Policy::InvalidPin => "Pinned",
        Policy::Banned => "Banned",
        Policy::Ignored => "Ignored",
        Policy::SameRange => "SameRange",
        Policy::SameMinor => "SameMinor",
        Policy::SnappedTo(_) => "SnappedTo",
        Policy::PreferredSemver(Prefer::Lowest) => "LowestSemver",
        Policy::PreferredSemver(_) => "HighestSemver",
    }
}

/// The valid state syncpack would name for an instance its group left alone.
fn valid_code(
    analysis: &Analysis<'_>,
    id: InstanceId,
    instance: &Instance<'_>,
    group: Option<&GroupSummary>,
) -> &'static str {
    if instance.specifier.is_catalog() {
        return "IsCatalog";
    }
    let Some(group) = group else {
        return "IsIdenticalToCatalog";
    };
    if matches!(group.policy, Policy::Ignored | Policy::InvalidPin) {
        return "IsIgnored";
    }
    if instance.is_own_version() {
        return "IsLocalAndValid";
    }
    let siblings: Vec<&Instance<'_>> = analysis
        .instances
        .named(&instance.name)
        .iter()
        .copied()
        .filter(|&other| other != id && analysis.owners[other.index()] == analysis.owners[id.index()])
        .map(|other| analysis.instances.get(other))
        .collect();
    let identical = siblings.iter().all(|other| other.raw() == instance.raw());
    let ranged = analysis.preferred[id.index()].is_some();
    match &group.policy {
        Policy::Pinned(_) => "IsIdenticalToPin",
        Policy::SameRange => "SatisfiesSameRangeGroup",
        Policy::SameMinor => "SatisfiesSameMinorGroup",
        Policy::SnappedTo(_) if ranged && !identical => "SatisfiesSnapTarget",
        Policy::SnappedTo(_) => "IsIdenticalToSnapTarget",
        _ => match siblings.iter().find(|other| other.is_own_version()) {
            Some(local) if local.raw() == instance.raw() => "IsIdenticalToLocal",
            Some(_) => "SatisfiesLocal",
            None if !has_version(&instance.specifier) => "IsNonSemverButIdentical",
            // A valid instance has the version the group aligns on; it only satisfies it when its semver group
            // gives it another range than the instances that set it.
            None if ranged && !identical => "SatisfiesHighestOrLowestSemver",
            None => "IsHighestOrLowestSemver",
        },
    }
}

fn has_version(specifier: &Arc<Specifier>) -> bool {
    specifier.get_node_version().is_some()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use skif_lint::{Registry, RunOptions, run_unsorted};

    use super::*;
    use crate::testing::{config, workspace};

    const ROOT: &str = r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0"}"#;

    /// `(group variant, dependency, package, status code, severity)` of every listed instance.
    fn listed(toml: &str, files: &[(&str, &str)]) -> Vec<(&'static str, String, String, String, Option<Severity>)> {
        let config = config(toml);
        let workspace = workspace(files, &config);
        let registry = Registry::new(crate::providers()).unwrap();
        let diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
        let enabled = registry.enabled(&config);
        inspect(&workspace, &config, &|id| enabled.contains(id), &diagnostics)
            .into_iter()
            .map(|status| {
                (
                    status.group.variant,
                    status.dependency,
                    status.package,
                    status.status_code,
                    status.severity,
                )
            })
            .collect()
    }

    fn row(
        variant: &'static str,
        dependency: &str,
        package: &str,
        code: &str,
        severity: Option<Severity>,
    ) -> (&'static str, String, String, String, Option<Severity>) {
        (
            variant,
            dependency.to_owned(),
            package.to_owned(),
            code.to_owned(),
            severity,
        )
    }

    #[test]
    fn every_governed_instance_is_listed_with_its_group_and_state() {
        let files = [
            ("package.json", ROOT),
            ("pnpm-workspace.yaml", "packages: [packages/*]\n"),
            (
                "packages/a/package.json",
                r#"{"name":"a","version":"1.0.0","dependencies":{"react":"18.3.1","zod":"3.0.0"},"peerDependencies":{"react":"^18"}}"#,
            ),
            (
                "packages/b/package.json",
                r#"{"name":"b","version":"1.0.0","dependencies":{"react":"18.2.0","zod":"3.1.0","a":"^1.0.0"}}"#,
            ),
        ];
        let toml = "[[version-groups]]\ndependencies = [\"zod\"]\npolicy = \"pinned\"\npin = \"3.1.0\"\n";
        assert_eq!(
            listed(toml, &files),
            [
                row("Pinned", "zod", "a", "DiffersToPin", Some(Severity::Error)),
                row("Pinned", "zod", "b", "IsIdenticalToPin", None),
                row("HighestSemver", "a", "a", "IsLocalAndValid", None),
                row("HighestSemver", "a", "b", "SatisfiesLocal", None),
                row("HighestSemver", "b", "b", "IsLocalAndValid", None),
                row("HighestSemver", "react", "a", "IsHighestOrLowestSemver", None),
                row(
                    "HighestSemver",
                    "react",
                    "b",
                    "DiffersToHighestOrLowestSemver",
                    Some(Severity::Error)
                ),
                row("HighestSemver", "root", "root", "IsLocalAndValid", None),
            ],
            "the peer range is outside the default scope and is not listed"
        );
    }

    #[test]
    fn catalog_states_come_from_the_catalog_rules() {
        let files = [
            ("package.json", ROOT),
            (
                "pnpm-workspace.yaml",
                "packages: [packages/*]\ncatalog:\n  react: 18.3.1\n",
            ),
            (
                "packages/a/package.json",
                r#"{"name":"a","version":"1.0.0","dependencies":{"react":"catalog:"}}"#,
            ),
            (
                "packages/b/package.json",
                r#"{"name":"b","version":"1.0.0","dependencies":{"react":"18.2.0"}}"#,
            ),
        ];
        let rows: Vec<_> = listed("", &files).into_iter().filter(|row| row.1 == "react").collect();
        assert_eq!(
            rows,
            [
                row("Catalog", "react", "a", "IsCatalog", None),
                row("Catalog", "react", "b", "DiffersToCatalog", Some(Severity::Warning)),
            ]
        );
    }

    #[test]
    fn ignored_groups_are_listed_as_ignored_and_a_rule_turned_off_reports_no_severity() {
        let files = [
            ("package.json", ROOT),
            ("pnpm-workspace.yaml", "packages: [packages/*]\n"),
            (
                "packages/a/package.json",
                r#"{"name":"a","version":"1.0.0","dependencies":{"react":"18.3.1","zod":"3.0.0"}}"#,
            ),
            (
                "packages/b/package.json",
                r#"{"name":"b","version":"1.0.0","dependencies":{"react":"18.2.0","zod":"3.1.0"}}"#,
            ),
        ];
        let toml = "[rules]\nversion-mismatch = \"off\"\n\n[[version-groups]]\ndependencies = [\"zod\"]\npolicy = \"ignored\"\n";
        let rows = listed(toml, &files);
        assert!(
            rows.contains(&row("Ignored", "zod", "a", "IsIgnored", None)),
            "{rows:?}"
        );
        assert!(
            rows.contains(&row(
                "HighestSemver",
                "react",
                "b",
                "DiffersToHighestOrLowestSemver",
                None
            )),
            "{rows:?}"
        );
    }

    #[test]
    fn status_types_follow_syncpack_classes() {
        assert_eq!(StatusType::of("IsIgnored"), StatusType::Valid);
        assert_eq!(StatusType::of("DiffersToPin"), StatusType::Fixable);
        assert_eq!(StatusType::of("SameRangeMismatch"), StatusType::Unfixable);
        assert_eq!(StatusType::of("MatchConflictsWithLocal"), StatusType::Conflict);
        assert_eq!(StatusType::of("RefuseToPinLocal"), StatusType::Suspect);
        assert_eq!(StatusType::of("DiffersToOtherCatalog"), StatusType::Unfixable);
    }
}
