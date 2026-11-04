//! The facts about a workspace that classify a one-sided syncpack finding as a known deviation: what the manifests
//! say, and which manifests syncpack read at all.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::diff::{Deviation, Side};

pub const NPM_ALIAS: &str = "npm-alias";
pub const MISSING_LOCAL_VERSION: &str = "missing-local-version";
pub const FLOATING_SPECIFIER: &str = "floating-specifier";
pub const SATISFIED_LOCAL_RANGE: &str = "satisfied-local-range";
pub const SHORTHAND_RANKING: &str = "shorthand-ranking";
pub const UNREAD_BY_SYNCPACK: &str = "unread-by-syncpack";

/// The intended differences between skif and syncpack ([ADR 0030](../../adrs/0030-harness-known-deviations-and-comparison-scope.md)).
pub const SYNCPACK_DEVIATIONS: [Deviation; 6] = [
    Deviation {
        id: SATISFIED_LOCAL_RANGE,
        reason: "a range the local version satisfies is valid for skif; syncpack requires byte equality with the local version",
    },
    Deviation {
        id: MISSING_LOCAL_VERSION,
        reason: "a package without a (valid) version is not a local instance for skif; syncpack reports InvalidLocalVersion and DependsOnInvalidLocalPackage",
    },
    Deviation {
        id: FLOATING_SPECIFIER,
        reason: "latest and * never become the alignment target for skif; syncpack ranks them highest and flags the instance against itself",
    },
    Deviation {
        id: UNREAD_BY_SYNCPACK,
        reason: "skif follows the workspace pattern semantics of the package manager; syncpack does not read trailing-slash or dot-directory patterns, so it never sees these packages",
    },
    Deviation {
        id: SHORTHAND_RANKING,
        reason: "skif ranks ranges by the lowest version they admit, so a shorthand such as ^14 never outranks ^14.1.2; syncpack lets the shorthand win",
    },
    Deviation {
        id: NPM_ALIAS,
        reason: "an npm: alias is a separate dependency for skif and is not aligned with the plain dependency",
    },
];

const DIFFERS_TO_LOCAL: &str = "DiffersToLocal";
const INVALID_LOCAL_VERSION: &str = "InvalidLocalVersion";
const DEPENDS_ON_INVALID_LOCAL_PACKAGE: &str = "DependsOnInvalidLocalPackage";
const DIFFERS_TO_HIGHEST_OR_LOWEST: &str = "DiffersToHighestOrLowestSemver";
const NON_SEMVER_MISMATCH: &str = "NonSemverMismatch";

const DEPENDENCY_SECTIONS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// What the classification reads from one `package.json`.
#[derive(Debug, Default, PartialEq)]
struct Manifest {
    name: Option<String>,
    version: Option<String>,
    specifiers: BTreeMap<String, Vec<String>>,
}

fn parse_manifest(text: &str) -> Option<Manifest> {
    let document: Value = serde_json::from_str(text).ok()?;
    let text_of = |key: &str| document.get(key).and_then(Value::as_str).map(str::to_owned);
    let mut specifiers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for section in DEPENDENCY_SECTIONS {
        for (name, specifier) in document.get(section).and_then(Value::as_object).into_iter().flatten() {
            if let Some(specifier) = specifier.as_str() {
                specifiers.entry(name.clone()).or_default().push(specifier.to_owned());
            }
        }
    }
    Some(Manifest {
        name: text_of("name"),
        version: text_of("version"),
        specifiers,
    })
}

/// `major.minor.patch` of a plain version; `None` for anything with a prerelease, build or other text.
fn triplet(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let mut next = || parts.next().map(str::parse::<u64>);
    let numbers = (next()?.ok()?, next()?.ok()?, next()?.ok()?);
    parts.next().is_none().then_some(numbers)
}

/// The numeric components of a range operand: `^8.5` is `[8, 5]`; `None` when it is not purely numeric.
fn components(operand: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = operand.split('.').map(|part| part.parse().ok()).collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 3)
}

fn padded(parts: &[u64]) -> (u64, u64, u64) {
    let at = |i: usize| parts.get(i).copied().unwrap_or(0);
    (at(0), at(1), at(2))
}

/// Whether the plain `version` satisfies the simple `range` (`*`, `^`, `~`, `>=`, an exact or partial version).
/// Anything more complex counts as not satisfied.
fn satisfies(range: &str, version: &str) -> bool {
    let range = range.trim();
    let Some(local) = triplet(version) else {
        return false;
    };
    if matches!(range, "*" | "x" | "X" | "") {
        return true;
    }
    let (operator, operand) = ["^", "~", ">=", "="]
        .iter()
        .find_map(|op| range.strip_prefix(op).map(|rest| (*op, rest)))
        .unwrap_or(("", range));
    let Some(parts) = components(operand.trim()) else {
        return false;
    };
    let base = padded(&parts);
    match operator {
        "^" => {
            local >= base
                && if base.0 > 0 {
                    local.0 == base.0
                } else if base.1 > 0 {
                    local.0 == 0 && local.1 == base.1
                } else {
                    local == base
                }
        }
        "~" => local >= base && local.0 == base.0 && (parts.len() == 1 || local.1 == base.1),
        ">=" => local >= base,
        _ => {
            let width = parts.len();
            padded_prefix(local, width) == padded_prefix(base, width)
        }
    }
}

fn padded_prefix(version: (u64, u64, u64), width: usize) -> (u64, u64, u64) {
    (
        version.0,
        if width > 1 { version.1 } else { 0 },
        if width > 2 { version.2 } else { 0 },
    )
}

/// A version syncpack's parser rejects: a prerelease tag with capitals (`0.0.0-PLACEHOLDER`).
fn invalid_for_syncpack(version: &str) -> bool {
    version
        .split_once('-')
        .is_some_and(|(_, prerelease)| prerelease.chars().any(|c| c.is_ascii_uppercase()))
}

/// The first numeric component of a range, for grouping ranges of one line.
fn leading_number(specifier: &str) -> Option<u64> {
    let bare = specifier.trim_start_matches(['^', '~', '>', '=', ' ']);
    bare.split('.').next()?.parse().ok()
}

fn is_shorthand(specifier: &str) -> bool {
    let bare = specifier.trim_start_matches(['^', '~', '>', '=', ' ']);
    components(bare).is_some_and(|parts| parts.len() < 3)
}

/// The manifests of a workspace and the set syncpack read.
#[derive(Debug, Default)]
pub struct Evidence {
    read: BTreeSet<String>,
    manifests: BTreeMap<String, Manifest>,
    /// Version (if any) of each package syncpack read, by package name.
    locals: BTreeMap<String, Option<String>>,
    /// Every specifier of a dependency across the packages syncpack read.
    by_dependency: BTreeMap<String, Vec<String>>,
}

impl Evidence {
    /// Reads the manifests syncpack read (`read`, relative to `root`) and those named by `keys`
    /// (`<package.json>|<dependency>`).
    pub fn load<'a>(root: &Path, read: BTreeSet<String>, keys: impl IntoIterator<Item = &'a String>) -> Self {
        let mut paths = read.clone();
        paths.extend(
            keys.into_iter()
                .filter_map(|key| key.split_once('|'))
                .map(|(path, _)| path.to_owned()),
        );
        let manifests = paths
            .into_iter()
            .filter_map(|path| {
                let manifest = fs::read_to_string(root.join(&path))
                    .ok()
                    .and_then(|t| parse_manifest(&t))?;
                Some((path, manifest))
            })
            .collect();
        Self::from_manifests(read, manifests)
    }

    fn from_manifests(read: BTreeSet<String>, manifests: BTreeMap<String, Manifest>) -> Self {
        let mut locals = BTreeMap::new();
        let mut by_dependency: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in &read {
            let Some(manifest) = manifests.get(path) else {
                continue;
            };
            if let Some(name) = &manifest.name {
                locals.insert(name.clone(), manifest.version.clone());
            }
            for (dependency, specifiers) in &manifest.specifiers {
                by_dependency
                    .entry(dependency.clone())
                    .or_default()
                    .extend(specifiers.iter().cloned());
            }
        }
        Self {
            read,
            manifests,
            locals,
            by_dependency,
        }
    }

    fn specifiers(&self, package: &str, dependency: &str) -> &[String] {
        self.manifests
            .get(package)
            .and_then(|m| m.specifiers.get(dependency))
            .map_or(&[][..], Vec::as_slice)
    }

    /// The known deviation a one-sided finding belongs to, if any. `status` is the syncpack status code and `key`
    /// is `<package.json>|<dependency>`.
    pub fn classify(&self, side: Side, status: &str, key: &str) -> Option<&'static str> {
        let (package, dependency) = key.split_once('|')?;
        let specifiers = self.specifiers(package, dependency);
        if specifiers.iter().any(|s| s.starts_with("npm:")) {
            return Some(NPM_ALIAS);
        }
        if side == Side::Skif && !self.read.contains(package) {
            return Some(UNREAD_BY_SYNCPACK);
        }
        match (side, status) {
            (Side::Tool, INVALID_LOCAL_VERSION) => {
                let version = self.manifests.get(package)?.version.as_deref();
                version
                    .is_none_or(invalid_for_syncpack)
                    .then_some(MISSING_LOCAL_VERSION)
            }
            (Side::Tool, DEPENDS_ON_INVALID_LOCAL_PACKAGE) => {
                let version = self.locals.get(dependency)?.as_deref();
                version
                    .is_none_or(invalid_for_syncpack)
                    .then_some(MISSING_LOCAL_VERSION)
            }
            (Side::Tool, DIFFERS_TO_LOCAL) => {
                let local = self.locals.get(dependency)?.as_deref()?;
                specifiers
                    .iter()
                    .any(|specifier| satisfies(specifier, local))
                    .then_some(SATISFIED_LOCAL_RANGE)
            }
            (_, DIFFERS_TO_HIGHEST_OR_LOWEST | NON_SEMVER_MISMATCH)
                if specifiers.iter().any(|s| matches!(s.trim(), "latest" | "*")) =>
            {
                Some(FLOATING_SPECIFIER)
            }
            (_, DIFFERS_TO_HIGHEST_OR_LOWEST) => self
                .shorthand_sibling(dependency, specifiers)
                .then_some(SHORTHAND_RANKING),
            _ => None,
        }
    }

    /// Whether the dependency also appears as a shorthand (`^14`) and as a full version (`^14.1.2`) of the line one
    /// of `specifiers` is on, which is where the two tools rank differently.
    fn shorthand_sibling(&self, dependency: &str, specifiers: &[String]) -> bool {
        let siblings = self.by_dependency.get(dependency).map_or(&[][..], Vec::as_slice);
        specifiers.iter().filter_map(|s| leading_number(s)).any(|line| {
            let on_line = |shorthand: bool| {
                siblings
                    .iter()
                    .any(|s| leading_number(s) == Some(line) && is_shorthand(s) == shorthand)
            };
            on_line(true) && on_line(false)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn satisfies_the_simple_ranges_only() {
        assert!(satisfies("^5.105.1", "5.105.1"));
        assert!(satisfies("^8.0.1", "8.0.10"));
        assert!(!satisfies("^8.0.11", "8.0.10"));
        assert!(!satisfies("^8.0.1", "9.0.0"));
        assert!(satisfies("^0.2.1", "0.2.5"));
        assert!(!satisfies("^0.2.1", "0.3.0"));
        assert!(satisfies("~1.2.0", "1.2.9"));
        assert!(!satisfies("~1.2.0", "1.3.0"));
        assert!(satisfies(">=1.0.0", "2.0.0"));
        assert!(satisfies("*", "0.0.0"));
        assert!(satisfies("1.2", "1.2.7"));
        assert!(!satisfies("1.2.3", "1.2.4"));
        assert!(!satisfies("^1 || ^2", "1.0.0"));
        assert!(!satisfies("workspace:*", "1.0.0"));
        assert!(!satisfies("^1.0.0", "1.0.0-beta.1"));
    }

    #[test]
    fn shorthand_detection() {
        assert!(is_shorthand("^14"));
        assert!(is_shorthand("~8.5"));
        assert!(!is_shorthand("^14.1.2"));
        assert!(!is_shorthand("latest"));
        assert_eq!(leading_number("^14.1.2"), Some(14));
    }

    #[test]
    fn capital_prerelease_tags_are_invalid_for_syncpack() {
        assert!(invalid_for_syncpack("0.0.0-PLACEHOLDER"));
        assert!(!invalid_for_syncpack("1.0.0-rc.1"));
        assert!(!invalid_for_syncpack("1.0.0"));
    }

    fn evidence() -> Evidence {
        let manifest = |name: &str, version: Option<&str>, specs: &[(&str, &str)]| Manifest {
            name: Some(name.to_owned()),
            version: version.map(str::to_owned),
            specifiers: specs
                .iter()
                .map(|(d, s)| ((*d).to_owned(), vec![(*s).to_owned()]))
                .collect(),
        };
        let manifests = BTreeMap::from([
            ("lib/package.json".to_owned(), manifest("lib", Some("5.1.0"), &[])),
            ("nover/package.json".to_owned(), manifest("nover", None, &[])),
            (
                "app/package.json".to_owned(),
                manifest(
                    "app",
                    Some("1.0.0"),
                    &[
                        ("lib", "^5.0.0"),
                        ("nover", "1.0.0"),
                        ("eslint", "latest"),
                        ("alias", "npm:other@^1"),
                        ("postcss", "^8.5.20"),
                    ],
                ),
            ),
            (
                "other/package.json".to_owned(),
                manifest("other", Some("1.0.0"), &[("postcss", "^8.5")]),
            ),
            ("unread/package.json".to_owned(), manifest("unread", Some("1.0.0"), &[])),
        ]);
        let read = ["lib", "nover", "app", "other"]
            .iter()
            .map(|d| format!("{d}/package.json"))
            .collect();
        Evidence::from_manifests(read, manifests)
    }

    #[test]
    fn classifies_each_syncpack_deviation_from_the_manifests() {
        let e = evidence();
        let c = |side, status, key| e.classify(side, status, key);
        assert_eq!(
            c(Side::Tool, "DiffersToLocal", "app/package.json|lib"),
            Some(SATISFIED_LOCAL_RANGE)
        );
        assert_eq!(c(Side::Skif, "DiffersToLocal", "app/package.json|lib"), None);
        assert_eq!(
            c(Side::Tool, "InvalidLocalVersion", "nover/package.json|nover"),
            Some(MISSING_LOCAL_VERSION)
        );
        assert_eq!(c(Side::Tool, "InvalidLocalVersion", "lib/package.json|lib"), None);
        assert_eq!(
            c(Side::Tool, "DependsOnInvalidLocalPackage", "app/package.json|nover"),
            Some(MISSING_LOCAL_VERSION)
        );
        assert_eq!(
            c(Side::Tool, "DiffersToHighestOrLowestSemver", "app/package.json|eslint"),
            Some(FLOATING_SPECIFIER)
        );
        assert_eq!(
            c(Side::Skif, "NonSemverMismatch", "app/package.json|eslint"),
            Some(FLOATING_SPECIFIER)
        );
        assert_eq!(
            c(Side::Tool, "DiffersToLocal", "app/package.json|alias"),
            Some(NPM_ALIAS)
        );
        assert_eq!(
            c(Side::Skif, "DiffersToLocal", "unread/package.json|lib"),
            Some(UNREAD_BY_SYNCPACK)
        );
        assert_eq!(c(Side::Tool, "DiffersToLocal", "unread/package.json|lib"), None);
        assert_eq!(
            c(Side::Skif, "DiffersToHighestOrLowestSemver", "app/package.json|postcss"),
            Some(SHORTHAND_RANKING)
        );
        assert_eq!(
            c(Side::Tool, "DiffersToHighestOrLowestSemver", "app/package.json|lib"),
            None
        );
    }
}
