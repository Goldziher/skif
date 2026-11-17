//! How far the type-aware analysis covered each package of a run (ADR 0010).
//!
//! The JS analysis resolves member reads with the TypeScript type checker where a package has usable types and falls
//! back to the name-based analysis elsewhere. A [`Report`](crate::Report) carries a [`TypeAwareSummary`] so a
//! consumer can tell a precise finding from a name-based one, and why a package was not typed.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The type-aware coverage of one run: whether it was on, the checker that ran, and every package's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct TypeAwareSummary {
    /// Whether the configuration asked for the type-aware analysis (`[js] type-aware`, `--type-aware`,
    /// `--no-type-aware`).
    pub enabled: bool,
    /// The type checker that ran and its version, when one ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Every analysed package, sorted by directory then name.
    #[serde(default)]
    pub packages: Vec<TypeAwarePackage>,
}

impl TypeAwareSummary {
    /// A run with the type-aware analysis `enabled` or not, covering `packages`, which are sorted by directory then
    /// name.
    #[must_use]
    pub fn new(enabled: bool, mut packages: Vec<TypeAwarePackage>) -> Self {
        sort(&mut packages);
        Self {
            enabled,
            engine: None,
            packages,
        }
    }

    /// Names the type checker that ran (`tsrs 7.1.0-beta.tsrs2`).
    #[must_use]
    pub fn with_engine(mut self, engine: impl Into<String>) -> Self {
        self.engine = Some(engine.into());
        self
    }

    /// The packages that are not fully typed: those [`Partial`](TypeAwareState::Partial) or
    /// [`Untyped`](TypeAwareState::Untyped).
    pub fn degraded(&self) -> impl Iterator<Item = &TypeAwarePackage> {
        self.packages
            .iter()
            .filter(|package| package.state != TypeAwareState::Typed)
    }

    /// The summaries of several roots as one: enabled when any root was, the first engine named, and every package,
    /// sorted. `None` when no root has a summary.
    #[must_use]
    pub fn merged<'a>(summaries: impl IntoIterator<Item = &'a Self>) -> Option<Self> {
        summaries.into_iter().fold(None, |merged: Option<Self>, summary| {
            let Some(mut merged) = merged else {
                return Some(summary.clone());
            };
            merged.enabled |= summary.enabled;
            if merged.engine.is_none() {
                merged.engine.clone_from(&summary.engine);
            }
            merged.packages.extend(summary.packages.iter().cloned());
            sort(&mut merged.packages);
            Some(merged)
        })
    }
}

fn sort(packages: &mut [TypeAwarePackage]) {
    packages.sort_by(|a, b| a.dir.cmp(&b.dir).then_with(|| a.name.cmp(&b.name)));
}

/// The type-aware coverage of one package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct TypeAwarePackage {
    /// The package directory, relative to the workspace root (empty for the root package).
    pub dir: PathBuf,
    /// The package name, or its directory when it has none.
    pub name: String,
    /// Whether its member reads were resolved with types.
    pub state: TypeAwareState,
    /// Why it is not fully typed: `disabled`, `not-built`, `no-tsconfig`, `tsconfig-error`, `not-typescript`,
    /// `unresolved-modules: N`, `files-outside-programs: N`, `checker-failed` or `cancelled`. `None` when typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// How many of its analysed files kept the name-based analysis.
    #[serde(default)]
    pub untyped_files: u32,
}

impl TypeAwarePackage {
    /// A package named `name` in `dir` in `state`, with no reason and no untyped file.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, name: impl Into<String>, state: TypeAwareState) -> Self {
        Self {
            dir: dir.into(),
            name: name.into(),
            state,
            reason: None,
            untyped_files: 0,
        }
    }

    /// Says why the package is not fully typed.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Sets how many of its files kept the name-based analysis.
    #[must_use]
    pub const fn with_untyped_files(mut self, untyped_files: u32) -> Self {
        self.untyped_files = untyped_files;
        self
    }

    /// The kind of [`reason`](Self::reason), without a count: `unresolved-modules` for `unresolved-modules: 3`.
    #[must_use]
    pub fn reason_kind(&self) -> Option<&str> {
        self.reason
            .as_deref()
            .map(|reason| reason.split_once(':').map_or(reason, |(kind, _)| kind).trim())
    }
}

/// How much of a package the type checker covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum TypeAwareState {
    /// Every analysed file was typed.
    Typed,
    /// Some files were typed, the others kept the name-based analysis.
    Partial,
    /// No file was typed.
    Untyped,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use super::*;

    fn package(dir: &str, name: &str, state: TypeAwareState) -> TypeAwarePackage {
        TypeAwarePackage::new(dir, name, state)
    }

    #[test]
    fn packages_are_sorted_by_directory_then_name() {
        let summary = TypeAwareSummary::new(
            true,
            vec![
                package("b", "b", TypeAwareState::Typed),
                package("a", "z", TypeAwareState::Typed),
                package("a", "y", TypeAwareState::Untyped),
            ],
        );
        let order: Vec<&str> = summary.packages.iter().map(|package| package.name.as_str()).collect();
        assert_eq!(order, ["y", "z", "b"]);
    }

    #[test]
    fn serializes_kebab_case_and_omits_what_is_unset() {
        let summary = TypeAwareSummary::new(
            true,
            vec![
                package("", "root", TypeAwareState::Typed),
                package("packages/a", "a", TypeAwareState::Partial)
                    .with_reason("unresolved-modules: 2")
                    .with_untyped_files(3),
            ],
        )
        .with_engine("tsrs 7.1.0-beta.tsrs2");
        assert_eq!(
            serde_json::to_value(&summary).unwrap(),
            serde_json::json!({
                "enabled": true,
                "engine": "tsrs 7.1.0-beta.tsrs2",
                "packages": [
                    {"dir": "", "name": "root", "state": "typed", "untyped-files": 0},
                    {
                        "dir": "packages/a",
                        "name": "a",
                        "state": "partial",
                        "reason": "unresolved-modules: 2",
                        "untyped-files": 3
                    }
                ]
            })
        );
        let disabled = serde_json::to_value(TypeAwareSummary::new(false, Vec::new())).unwrap();
        assert_eq!(disabled, serde_json::json!({"enabled": false, "packages": []}));
    }

    #[test]
    fn round_trips_through_json() {
        let summary = TypeAwareSummary::new(
            true,
            vec![package("a", "a", TypeAwareState::Untyped).with_reason("no-tsconfig")],
        );
        let text = serde_json::to_string(&summary).unwrap();
        assert_eq!(serde_json::from_str::<TypeAwareSummary>(&text).unwrap(), summary);
    }

    #[test]
    fn the_reason_kind_drops_the_count() {
        let kind = |reason: &str| {
            package("a", "a", TypeAwareState::Partial)
                .with_reason(reason)
                .reason_kind()
                .map(str::to_owned)
        };
        assert_eq!(kind("unresolved-modules: 3").as_deref(), Some("unresolved-modules"));
        assert_eq!(kind("no-tsconfig").as_deref(), Some("no-tsconfig"));
        assert_eq!(package("a", "a", TypeAwareState::Typed).reason_kind(), None);
    }

    #[test]
    fn degraded_lists_partial_and_untyped_packages() {
        let summary = TypeAwareSummary::new(
            true,
            vec![
                package("a", "a", TypeAwareState::Typed),
                package("b", "b", TypeAwareState::Partial),
                package("c", "c", TypeAwareState::Untyped),
            ],
        );
        let names: Vec<&str> = summary.degraded().map(|package| package.name.as_str()).collect();
        assert_eq!(names, ["b", "c"]);
    }

    #[test]
    fn merging_combines_roots() {
        let first = TypeAwareSummary::new(false, vec![package("z", "z", TypeAwareState::Untyped)]);
        let second = TypeAwareSummary::new(true, vec![package("a", "a", TypeAwareState::Typed)]).with_engine("tsrs");
        let merged = TypeAwareSummary::merged([&first, &second]).unwrap();
        assert!(merged.enabled);
        assert_eq!(merged.engine.as_deref(), Some("tsrs"));
        let dirs: Vec<&Path> = merged.packages.iter().map(|package| package.dir.as_path()).collect();
        assert_eq!(dirs, [Path::new("a"), Path::new("z")]);
        assert_eq!(TypeAwareSummary::merged(Vec::<&TypeAwareSummary>::new()), None);
    }
}
