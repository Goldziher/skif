//! `skif update`: newer published versions for the dependencies the version policies govern (ADR 0050).
//!
//! The module is pure. The package registry is reached through the [`Registry`] trait, which the host implements:
//! the `skif` command line does HTTP, a test or an embedding host answers from memory. [`plan_updates`] collects the
//! instances that may move, asks the registry once per dependency name, and returns one [`Update`] per instance that
//! can move, each with the [`Fix`] that writes it. Nothing here reads the clock: the caller passes the time
//! `minimum-release-age` is measured from.
//!
//! Which instances move follows syncpack's `update` (15.3.3): an instance of a dependency whose version group prefers
//! the highest version, that is not a package of the workspace and has no `catalog:` sibling in its group, written as
//! an exact version, a range over one version, a bare major or minor, or `latest`. The catalog entries of
//! `pnpm-workspace.yaml`, the root `package.json` (bun) and `.yarnrc.yml` move the same way. The first
//! `[[update-groups]]` entry that matches an instance caps its target or leaves it alone.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use skif_config::{Config, Prefer, Selector, UpdateGroup, UpdateTarget};
use skif_core::{Fix, FixOp, FixSafety, Location};
use skif_specifier::Specifier;
use skif_specifier::semver_range::SemverRange;
use skif_specifier::update_target::UpdateTarget as Reach;
use skif_workspace::Workspace;

use crate::policy::catalog::names::{CatalogName, defined_catalogs};
use crate::policy::catalog::target::{self, CatalogFile};
use crate::policy::catalog::{UsageIndex, owned_dependencies};
use crate::policy::groups::{Groups, Policy, Selectors, identifier_matches};
use crate::policy::instances::{Instance, Instances};

/// The syncpack state of an instance a newer published version can replace.
pub const STATE: &str = "DiffersToNpmRegistry";

/// Minutes a version must have been published before it is proposed when neither `[update] minimum-release-age` nor
/// `pnpm-workspace.yaml` says otherwise: one day, as in syncpack.
pub const DEFAULT_MINIMUM_RELEASE_AGE: u64 = 1440;

/// One published version of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Published {
    /// The version, as the registry lists it.
    pub version: String,
    /// When it was published, in seconds since the Unix epoch, when the registry says.
    pub published_at: Option<i64>,
    /// Whether the version is deprecated; deprecated versions are never proposed.
    pub deprecated: bool,
}

impl Published {
    /// A version published at `published_at` (Unix seconds), not deprecated.
    #[must_use]
    pub fn new(version: impl Into<String>, published_at: Option<i64>) -> Self {
        Self {
            version: version.into(),
            published_at,
            deprecated: false,
        }
    }

    /// Marks the version deprecated.
    #[must_use]
    pub const fn deprecated(mut self) -> Self {
        self.deprecated = true;
        self
    }
}

/// What the registry knows about one package: every published version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Packument {
    /// The published versions, in any order.
    pub versions: Vec<Published>,
}

#[derive(Deserialize)]
struct RawPackument {
    #[serde(default)]
    versions: BTreeMap<String, RawVersion>,
    #[serde(default)]
    time: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RawVersion {
    #[serde(default)]
    deprecated: Option<serde_json::Value>,
}

impl Packument {
    /// A packument of `versions`.
    #[must_use]
    pub const fn new(versions: Vec<Published>) -> Self {
        Self { versions }
    }

    /// Reads an npm registry document (`GET /<name>` with `Accept: application/json`): the keys of `versions`, a
    /// `deprecated` message on a version, and the publish times of `time`.
    ///
    /// # Errors
    /// [`FetchError::Invalid`] when the text is not such a document.
    pub fn from_json(text: &str) -> Result<Self, FetchError> {
        let raw: RawPackument = serde_json::from_str(text).map_err(|error| FetchError::Invalid(error.to_string()))?;
        let versions = raw
            .versions
            .into_iter()
            .map(|(version, meta)| {
                let deprecated = match &meta.deprecated {
                    None | Some(serde_json::Value::Null | serde_json::Value::Bool(false)) => false,
                    Some(serde_json::Value::String(message)) => !message.is_empty(),
                    Some(_) => true,
                };
                let published_at = raw.time.get(&version).and_then(|time| unix_seconds(time));
                Published {
                    version,
                    published_at,
                    deprecated,
                }
            })
            .collect();
        Ok(Self { versions })
    }
}

/// Seconds since the Unix epoch of an RFC 3339 time: `2024-01-15T10:30:00.000Z` as the npm registry writes it, or
/// with a numeric offset (`+02:00`, `-05:30`) as other registries may; `None` for anything else. The calendar
/// arithmetic is Howard Hinnant's `days_from_civil`.
#[must_use]
pub fn unix_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't') {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut zone = &text[19..];
    if let Some(fraction) = zone.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        zone = &fraction[digits..];
    }
    let offset = match zone.as_bytes() {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let at = text.len() - 5;
            let (hours, minutes) = (number(at..at + 2)?, number(at + 3..at + 5)?);
            if hours > 23 || minutes > 59 {
                return None;
            }
            let offset = hours * 3_600 + minutes * 60;
            if *sign == b'-' { -offset } else { offset }
        }
        _ => return None,
    };
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

/// Why the registry could not say which versions of a package exist.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchError {
    /// The registry has no such package (HTTP 404), or will not show it without credentials.
    NotFound,
    /// The registry answered with another unsuccessful status.
    Status(u16),
    /// No answer: name resolution, connection, TLS or a timeout. The message never holds a credential.
    Network(String),
    /// The answer is not a registry document.
    Invalid(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found in the registry"),
            Self::Status(status) => write!(f, "the registry answered HTTP {status}"),
            Self::Network(message) => write!(f, "cannot reach the registry: {message}"),
            Self::Invalid(message) => write!(f, "the registry sent an unreadable document: {message}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// The package registry, as the host reaches it.
pub trait Registry: Sync {
    /// Every published version of package `name`.
    ///
    /// # Errors
    /// A [`FetchError`] when the registry cannot say.
    fn fetch(&self, name: &str) -> Result<Packument, FetchError>;

    /// [`fetch`](Self::fetch) for each of `names`, answers in the same order. The default asks one after the other; a
    /// host overrides it to ask several at once.
    fn fetch_all(&self, names: &[&str]) -> Vec<Result<Packument, FetchError>> {
        names.iter().map(|name| self.fetch(name)).collect()
    }
}

/// What a run of [`plan_updates`] looks for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UpdateOptions {
    /// The furthest any version may move (`--target`); an update group can only narrow it.
    pub target: UpdateTarget,
    /// Now, in seconds since the Unix epoch: `minimum-release-age` is counted back from here.
    pub now: i64,
    /// Only these dependencies (`--dependencies`); empty means every dependency.
    pub dependencies: Selector,
}

impl UpdateOptions {
    /// Every dependency, up to the latest version, at `now` (Unix seconds).
    #[must_use]
    pub fn new(now: i64) -> Self {
        Self {
            target: UpdateTarget::Latest,
            now,
            dependencies: Selector::default(),
        }
    }

    /// Sets the furthest a version may move.
    #[must_use]
    pub const fn with_target(mut self, target: UpdateTarget) -> Self {
        self.target = target;
        self
    }

    /// Only the dependencies `dependencies` selects.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: Selector) -> Self {
        self.dependencies = dependencies;
        self
    }
}

/// One declared version that a newer published version can replace.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Update {
    /// The dependency name.
    pub dependency: String,
    /// The package that declares it; `None` for a catalog entry.
    pub package: Option<String>,
    /// Where it is declared.
    pub location: Location,
    /// The place: a dependency section (`devDependencies`), an override section, a custom type, `catalog` or
    /// `catalogs.<name>`.
    pub place: String,
    /// The specifier as written.
    pub current: String,
    /// The specifier that replaces it, in the same range style or the one a semver group prefers.
    pub target: String,
    /// Writes `target` over `current`. Unsafe: it changes what is installed.
    pub fix: Fix,
}

/// A dependency the registry could not answer for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Failure {
    /// The package name that was asked for.
    pub dependency: String,
    /// What went wrong.
    pub error: FetchError,
}

/// Something a run of [`plan_updates`] left out that is not a registry failure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Note {
    /// Not a name the npm registry can hold, so it is not looked up.
    InvalidName {
        /// The name as written.
        dependency: String,
    },
    /// The registry gives no usable publish time for `version`, which would otherwise be the target; with a minimum
    /// release age it cannot be shown to be old enough, so it is not proposed.
    NoPublishTime {
        /// The dependency name.
        dependency: String,
        /// The version left out.
        version: String,
    },
}

impl Note {
    /// The dependency the note is about.
    #[must_use]
    pub fn dependency(&self) -> &str {
        match self {
            Self::InvalidName { dependency } | Self::NoPublishTime { dependency, .. } => dependency,
        }
    }
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { .. } => f.write_str("not a valid npm package name, so it is not looked up"),
            Self::NoPublishTime { version, .. } => write!(
                f,
                "{version} is not proposed: the registry gives no publish time to check the minimum release age against"
            ),
        }
    }
}

/// The result of [`plan_updates`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct UpdateReport {
    /// The updates, sorted by file, position and dependency.
    pub updates: Vec<Update>,
    /// The packages the registry could not answer for, by name.
    pub failures: Vec<Failure>,
    /// What was left out for another reason, sorted.
    pub notes: Vec<Note>,
    /// How many packages the registry was asked about.
    pub queried: usize,
    /// The minimum release age that applied, in minutes.
    pub minimum_release_age: u64,
}

/// The minutes a version must have been published before it is proposed: `[update] minimum-release-age`, else the
/// `minimumReleaseAge` of `pnpm-workspace.yaml`, else [`DEFAULT_MINIMUM_RELEASE_AGE`].
#[must_use]
pub fn minimum_release_age(workspace: &Workspace, config: &Config) -> u64 {
    config
        .update
        .minimum_release_age
        .or_else(|| workspace.pnpm_workspace.as_ref()?.minimum_release_age)
        .unwrap_or(DEFAULT_MINIMUM_RELEASE_AGE)
}

/// Where an update is written.
enum Writer<'w> {
    Instance(Instance<'w>),
    Catalog { file: CatalogFile, catalog: CatalogName },
}

/// An instance that may move, before the registry is asked.
struct Candidate<'w> {
    dependency: String,
    package: Option<String>,
    place: String,
    specifier: Arc<Specifier>,
    target: UpdateTarget,
    range: Option<SemverRange>,
    location: Location,
    writer: Writer<'w>,
}

/// What the first matching update group says about an instance.
enum Cap {
    Skip,
    Up(UpdateTarget),
}

fn cap(group: Option<&UpdateGroup>, run: UpdateTarget) -> Cap {
    match group {
        Some(group) if group.ignored => Cap::Skip,
        Some(group) => Cap::Up(group.target.map_or(run, |target| run.stricter(target))),
        None => Cap::Up(run),
    }
}

/// The specifiers syncpack asks the registry about: an exact version, a range over one version, a bare major or
/// minor, `latest`, and an `npm:` alias of the dependency itself.
fn updatable(specifier: &Specifier, name: &str) -> bool {
    match specifier {
        Specifier::Exact(_)
        | Specifier::Range(_)
        | Specifier::Major(_)
        | Specifier::Minor(_)
        | Specifier::Latest(_) => true,
        Specifier::Alias(alias) => alias.name == name && updatable(&alias.inner_specifier, name),
        _ => false,
    }
}

/// The longest package name npm accepts, scope included (`validate-npm-package-name`).
const MAX_PACKAGE_NAME_LEN: usize = 214;

/// Whether `name` is a name the npm registry can hold: `^(@[a-z0-9-~][a-z0-9-._~]*/)?[a-z0-9-~][a-z0-9-._~]*$`,
/// ignoring case because the registry still serves legacy names with capitals. Only such a name is put in a registry
/// URL, so a name from a manifest cannot add a path segment, a query or a fragment to it. npm also caps a name at 214
/// characters, scope included.
#[must_use]
pub fn is_valid_package_name(name: &str) -> bool {
    fn segment(text: &str) -> bool {
        let mut bytes = text.bytes();
        bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric() || matches!(first, b'-' | b'~'))
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    }
    if name.len() > MAX_PACKAGE_NAME_LEN {
        return false;
    }
    match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, rest)| segment(scope) && segment(rest)),
        None => segment(name),
    }
}

/// Whether `name` is a plain package name: not an override selector such as `a>b` or `json5@<1.0.2`.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('>') && name.rfind('@').unwrap_or(0) == 0
}

fn instance_candidates<'w>(
    workspace: &'w Workspace,
    config: &Config,
    options: &UpdateOptions,
    instances: &Instances<'w>,
    out: &mut Vec<Candidate<'w>>,
) {
    let groups = Groups::compile(workspace, config);
    let assignment = groups.assign(instances);
    let scope = &config.policy.dependency_types;
    let update_groups: Vec<(Selectors<'_>, &UpdateGroup)> = config
        .update_groups
        .iter()
        .map(|group| {
            let selectors = Selectors::new(
                workspace,
                &group.dependencies,
                &group.packages,
                &group.dependency_types,
                scope,
                &group.specifier_types,
            );
            (selectors, group)
        })
        .collect();
    for (index, group) in groups.groups().iter().enumerate() {
        if !matches!(group.policy, Policy::PreferredSemver(Prefer::Highest)) {
            continue;
        }
        for (name, ids) in assignment.dependencies(index) {
            if !options.dependencies.is_empty() && !options.dependencies.matches(name, false) {
                continue;
            }
            // The catalog wins over the registry: its entry is what moves.
            if ids
                .iter()
                .any(|&id| matches!(*instances.get(id).specifier, Specifier::Catalog(_)))
            {
                continue;
            }
            for &id in ids {
                let instance = instances.get(id);
                if instance.is_local
                    || instance.is_own_version()
                    || !plain_name(&instance.name)
                    || !updatable(&instance.specifier, &instance.name)
                {
                    continue;
                }
                let group = update_groups
                    .iter()
                    .find(|(selectors, _)| selectors.matches(instance))
                    .map(|(_, group)| *group);
                let Cap::Up(target) = cap(group, options.target) else {
                    continue;
                };
                out.push(Candidate {
                    dependency: instance.name.clone(),
                    package: instance.package.name.clone(),
                    place: instance.ty().label().to_owned(),
                    specifier: Arc::clone(&instance.specifier),
                    target,
                    range: assignment.preferred(id).cloned(),
                    location: instance.location(),
                    writer: Writer::Instance(instance.clone()),
                });
            }
        }
    }
}

fn catalog_candidates<'w>(
    workspace: &'w Workspace,
    config: &Config,
    options: &UpdateOptions,
    local: &BTreeSet<&str>,
    out: &mut Vec<Candidate<'w>>,
) {
    let catalogs = &workspace.catalogs;
    for catalog in defined_catalogs(catalogs) {
        let Some(file) = target::existing_file(workspace, &catalog) else {
            continue;
        };
        let (entries, place) = match &catalog {
            CatalogName::Default => (catalogs.default_catalog(), "catalog".to_owned()),
            CatalogName::Named(name) => match catalogs.named().get(name) {
                Some(entries) => (entries, format!("catalogs.{name}")),
                None => continue,
            },
        };
        for (name, version) in entries {
            if local.contains(name.as_str())
                || !plain_name(name)
                || (!options.dependencies.is_empty() && !options.dependencies.matches(name, false))
            {
                continue;
            }
            let specifier = Specifier::new(version);
            if !updatable(&specifier, name) {
                continue;
            }
            // A catalog entry belongs to no package and has no dependency type: only groups that select neither apply.
            let group = config.update_groups.iter().find(|group| {
                group.packages.is_empty()
                    && group.dependency_types.is_empty()
                    && (group.dependencies.is_empty() || group.dependencies.matches(name, false))
                    && identifier_matches(&group.specifier_types, specifier.get_config_identifier())
            });
            let Cap::Up(reach) = cap(group, options.target) else {
                continue;
            };
            out.push(Candidate {
                dependency: name.clone(),
                package: None,
                place: place.clone(),
                specifier,
                target: reach,
                range: None,
                location: Location::new(target::file_path(&file).clone(), None),
                writer: Writer::Catalog {
                    file: file.clone(),
                    catalog: catalog.clone(),
                },
            });
        }
    }
}

const fn reach(target: UpdateTarget) -> Reach {
    match target {
        UpdateTarget::Minor => Reach::Minor,
        UpdateTarget::Patch => Reach::Patch,
        _ => Reach::Latest,
    }
}

/// The versions of a package that may be proposed, highest first.
#[derive(Default)]
struct Proposable {
    /// Not deprecated, valid semver, and published before the cutoff when there is one.
    eligible: Vec<Arc<Specifier>>,
    /// As `eligible`, plus the versions without a usable publish time that the cutoff excludes: what would be
    /// eligible if the registry said when they were published. Empty when no version was left out that way.
    with_unknown_times: Vec<Arc<Specifier>>,
}

fn highest_first(versions: &mut [Arc<Specifier>]) {
    versions.sort_by_cached_key(|specifier| std::cmp::Reverse(specifier.get_node_version()));
}

/// The versions that may be proposed. With a cutoff, a version the registry gives no publish time for is not
/// eligible: it cannot be shown to be old enough.
fn proposable(packument: &Packument, cutoff: Option<i64>) -> Proposable {
    let mut eligible = Vec::new();
    let mut unknown = Vec::new();
    for published in packument.versions.iter().filter(|published| !published.deprecated) {
        let specifier = Specifier::new(&published.version);
        if specifier.get_node_version().is_none() {
            continue;
        }
        match (cutoff, published.published_at) {
            (None, _) => eligible.push(specifier),
            (Some(cutoff), Some(time)) if time <= cutoff => eligible.push(specifier),
            (Some(_), Some(_)) => {}
            (Some(_), None) => unknown.push(specifier),
        }
    }
    let with_unknown_times = if unknown.is_empty() {
        Vec::new()
    } else {
        let mut all = eligible.clone();
        all.append(&mut unknown);
        highest_first(&mut all);
        all
    };
    highest_first(&mut eligible);
    Proposable {
        eligible,
        with_unknown_times,
    }
}

/// The specifier `candidate` moves to, if any published version is eligible.
fn proposal(candidate: &Candidate<'_>, highest_first: &[Arc<Specifier>]) -> Option<String> {
    let installed = &candidate.specifier;
    let reach = reach(candidate.target);
    let newer = highest_first.iter().find(|update| {
        update.is_eligible_update_for(installed, &reach) && installed.has_same_release_channel_as(update)
    })?;
    let range = candidate
        .range
        .clone()
        .or_else(|| installed.get_semver_range())
        .unwrap_or(SemverRange::Exact);
    let version = newer.get_node_version()?;
    let moved = installed.with_node_version(&version)?.with_range(&range)?;
    let raw = moved.get_raw();
    (raw != installed.get_raw()).then(|| raw.to_owned())
}

fn write_op(workspace: &Workspace, candidate: &Candidate<'_>, specifier: &str) -> Option<FixOp> {
    match &candidate.writer {
        Writer::Instance(instance) => instance.set_specifier(specifier),
        Writer::Catalog { file, catalog } => Some(target::set_entry(
            workspace,
            file,
            catalog,
            &candidate.dependency,
            specifier,
        )),
    }
}

/// Finds every declared version a newer published version can replace (ADR 0050).
///
/// The registry is asked once for each distinct dependency name among the instances that may move, through
/// [`Registry::fetch_all`]; a name that is not [a valid package name](is_valid_package_name) is not asked about and
/// gets a [`Note`], and a name it cannot answer for becomes a [`Failure`] and its instances are left out. Versions
/// published less than [`minimum_release_age`] minutes before `options.now`, and deprecated versions, are never
/// proposed. With a minimum release age, neither is a version the registry gives no publish time for; when such a
/// version would have been the target, a [`Note`] says so.
#[must_use]
pub fn plan_updates(
    workspace: &Workspace,
    config: &Config,
    registry: &dyn Registry,
    options: &UpdateOptions,
) -> UpdateReport {
    let usages = UsageIndex::build(workspace, config);
    let owned = owned_dependencies(&usages, workspace, &config.catalog);
    let instances = Instances::collect_with(workspace, &owned, &config.custom_types, config.policy.override_reading);
    let mut candidates = Vec::new();
    instance_candidates(workspace, config, options, &instances, &mut candidates);
    let local: BTreeSet<&str> = workspace
        .all_packages()
        .filter_map(|package| package.name.as_deref())
        .collect();
    catalog_candidates(workspace, config, options, &local, &mut candidates);
    let mut notes = BTreeSet::new();
    candidates.retain(|candidate| {
        let valid = is_valid_package_name(&candidate.dependency);
        if !valid {
            notes.insert(Note::InvalidName {
                dependency: candidate.dependency.clone(),
            });
        }
        valid
    });

    let names: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.dependency.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let age = minimum_release_age(workspace, config);
    let cutoff = (age > 0).then(|| {
        let seconds = i64::try_from(age.saturating_mul(60)).unwrap_or(i64::MAX);
        options.now.saturating_sub(seconds)
    });
    let mut known: BTreeMap<&str, Proposable> = BTreeMap::new();
    let mut failures = Vec::new();
    let answers = registry.fetch_all(&names);
    for (index, &name) in names.iter().enumerate() {
        match answers.get(index) {
            Some(Ok(packument)) => {
                known.insert(name, proposable(packument, cutoff));
            }
            Some(Err(error)) => failures.push(Failure {
                dependency: name.to_owned(),
                error: error.clone(),
            }),
            None => failures.push(Failure {
                dependency: name.to_owned(),
                error: FetchError::Invalid("the registry returned no answer".to_owned()),
            }),
        }
    }

    let mut updates: Vec<Update> = candidates
        .iter()
        .filter_map(|candidate| {
            let versions = known.get(candidate.dependency.as_str())?;
            let target = proposal(candidate, &versions.eligible);
            if !versions.with_unknown_times.is_empty() {
                let unchecked = proposal(candidate, &versions.with_unknown_times);
                if let Some(unchecked) = unchecked.filter(|unchecked| Some(unchecked) != target.as_ref()) {
                    let version = Specifier::new(&unchecked)
                        .get_node_version()
                        .map_or(unchecked, |version| version.to_string());
                    notes.insert(Note::NoPublishTime {
                        dependency: candidate.dependency.clone(),
                        version,
                    });
                }
            }
            let target = target?;
            let op = write_op(workspace, candidate, &target)?;
            Some(Update {
                dependency: candidate.dependency.clone(),
                package: candidate.package.clone(),
                location: candidate.location.clone(),
                place: candidate.place.clone(),
                current: candidate.specifier.get_raw().to_owned(),
                fix: Fix::new(
                    format!("Update {} to {target}", candidate.dependency),
                    FixSafety::Unsafe,
                    vec![op],
                ),
                target,
            })
        })
        .collect();
    updates.sort_by(|a, b| {
        let start = |update: &Update| update.location.span.map(|span| span.start);
        (&a.location.path, start(a), &a.dependency, &a.place).cmp(&(
            &b.location.path,
            start(b),
            &b.dependency,
            &b.place,
        ))
    });
    UpdateReport {
        updates,
        failures,
        notes: notes.into_iter().collect(),
        queried: names.len(),
        minimum_release_age: age,
    }
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<UpdateReport>;
    let _ = assert_send_sync::<UpdateOptions>;
    let _ = assert_send_sync::<FetchError>;
};

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
