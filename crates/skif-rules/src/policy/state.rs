//! The instance states a version or semver group can assign, and the rule each one is reported under.
//!
//! The names are syncpack's (ADR 0009) and travel in `Diagnostic.detail`. States that mean "valid" have no variant:
//! an instance without a [`Verdict`] is valid.

use std::sync::Arc;

use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_specifier::Specifier;

/// Metadata of `version-mismatch`.
pub static VERSION_MISMATCH: RuleMeta = RuleMeta::new(
    "version-mismatch",
    Category::Policy,
    Some(Severity::Error),
    Fixability::Sometimes,
    "A dependency instance differs from the version its version group requires (highest, lowest, pinned, local or snapped).",
)
.version_scoped();

/// Metadata of `banned-dependency`.
pub static BANNED_DEPENDENCY: RuleMeta = RuleMeta::new(
    "banned-dependency",
    Category::Policy,
    Some(Severity::Error),
    Fixability::Unsafe,
    "A dependency is declared, or resolved by the lockfile, although a banned version group forbids it.",
);

/// Metadata of `semver-range-mismatch`.
pub static SEMVER_RANGE_MISMATCH: RuleMeta = RuleMeta::new(
    "semver-range-mismatch",
    Category::Policy,
    Some(Severity::Error),
    Fixability::Unsafe,
    "A dependency uses a semver range other than the one its semver group requires.",
)
.version_scoped();

/// Metadata of `policy-conflict`.
pub static POLICY_CONFLICT: RuleMeta = RuleMeta::new(
    "policy-conflict",
    Category::Policy,
    Some(Severity::Warning),
    Fixability::Sometimes,
    "Two policies require incompatible outcomes for one dependency, or a group cannot be applied as configured.",
)
.version_scoped();

/// Metadata of `invalid-local-version`.
pub static INVALID_LOCAL_VERSION: RuleMeta = RuleMeta::new(
    "invalid-local-version",
    Category::Policy,
    Some(Severity::Error),
    Fixability::None,
    "A workspace package has a `version` that is not an exact semver version, so dependents cannot align with it.",
);

/// A non-valid state of a dependency instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum State {
    IsBanned,
    DiffersToHighestOrLowestSemver,
    DiffersToLocal,
    DiffersToSnapTarget,
    DiffersToPin,
    SemverRangeMismatch,
    PinOverridesSemverRange,
    PinOverridesSemverRangeMismatch,
    SameMinorOverridesSemverRange,
    SameMinorOverridesSemverRangeMismatch,
    NonSemverMismatch,
    SameRangeMismatch,
    SameMinorMismatch,
    SameMinorHasMajorMismatch,
    DependsOnInvalidLocalPackage,
    MatchConflictsWithHighestOrLowestSemver,
    MismatchConflictsWithHighestOrLowestSemver,
    MatchConflictsWithSnapTarget,
    MismatchConflictsWithSnapTarget,
    MatchConflictsWithLocal,
    MismatchConflictsWithLocal,
    InvalidLocalVersion,
    RefuseToBanLocal,
    RefuseToPinLocal,
    RefuseToSnapLocal,
    DependsOnMissingSnapTarget,
    /// skif-only: a configured group that owns no instance.
    UnusedGroup,
    /// skif-only: a `pinned` group whose `pin` is empty or not a version specifier.
    InvalidPin,
}

impl State {
    /// The syncpack state name, as carried in `Diagnostic.detail`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::IsBanned => "IsBanned",
            Self::DiffersToHighestOrLowestSemver => "DiffersToHighestOrLowestSemver",
            Self::DiffersToLocal => "DiffersToLocal",
            Self::DiffersToSnapTarget => "DiffersToSnapTarget",
            Self::DiffersToPin => "DiffersToPin",
            Self::SemverRangeMismatch => "SemverRangeMismatch",
            Self::PinOverridesSemverRange => "PinOverridesSemverRange",
            Self::PinOverridesSemverRangeMismatch => "PinOverridesSemverRangeMismatch",
            Self::SameMinorOverridesSemverRange => "SameMinorOverridesSemverRange",
            Self::SameMinorOverridesSemverRangeMismatch => "SameMinorOverridesSemverRangeMismatch",
            Self::NonSemverMismatch => "NonSemverMismatch",
            Self::SameRangeMismatch => "SameRangeMismatch",
            Self::SameMinorMismatch => "SameMinorMismatch",
            Self::SameMinorHasMajorMismatch => "SameMinorHasMajorMismatch",
            Self::DependsOnInvalidLocalPackage => "DependsOnInvalidLocalPackage",
            Self::MatchConflictsWithHighestOrLowestSemver => "MatchConflictsWithHighestOrLowestSemver",
            Self::MismatchConflictsWithHighestOrLowestSemver => "MismatchConflictsWithHighestOrLowestSemver",
            Self::MatchConflictsWithSnapTarget => "MatchConflictsWithSnapTarget",
            Self::MismatchConflictsWithSnapTarget => "MismatchConflictsWithSnapTarget",
            Self::MatchConflictsWithLocal => "MatchConflictsWithLocal",
            Self::MismatchConflictsWithLocal => "MismatchConflictsWithLocal",
            Self::InvalidLocalVersion => "InvalidLocalVersion",
            Self::RefuseToBanLocal => "RefuseToBanLocal",
            Self::RefuseToPinLocal => "RefuseToPinLocal",
            Self::RefuseToSnapLocal => "RefuseToSnapLocal",
            Self::DependsOnMissingSnapTarget => "DependsOnMissingSnapTarget",
            Self::UnusedGroup => "UnusedGroup",
            Self::InvalidPin => "InvalidPin",
        }
    }

    /// The rule this state is reported under (ADR 0009).
    pub(crate) fn rule(self) -> &'static RuleMeta {
        match self {
            Self::IsBanned => &BANNED_DEPENDENCY,
            Self::DiffersToHighestOrLowestSemver
            | Self::DiffersToLocal
            | Self::DiffersToSnapTarget
            | Self::DiffersToPin
            | Self::NonSemverMismatch
            | Self::SameRangeMismatch
            | Self::SameMinorMismatch
            | Self::SameMinorHasMajorMismatch => &VERSION_MISMATCH,
            Self::SemverRangeMismatch => &SEMVER_RANGE_MISMATCH,
            Self::DependsOnInvalidLocalPackage | Self::InvalidLocalVersion => &INVALID_LOCAL_VERSION,
            Self::PinOverridesSemverRange
            | Self::PinOverridesSemverRangeMismatch
            | Self::SameMinorOverridesSemverRange
            | Self::SameMinorOverridesSemverRangeMismatch
            | Self::MatchConflictsWithHighestOrLowestSemver
            | Self::MismatchConflictsWithHighestOrLowestSemver
            | Self::MatchConflictsWithSnapTarget
            | Self::MismatchConflictsWithSnapTarget
            | Self::MatchConflictsWithLocal
            | Self::MismatchConflictsWithLocal
            | Self::RefuseToBanLocal
            | Self::RefuseToPinLocal
            | Self::RefuseToSnapLocal
            | Self::DependsOnMissingSnapTarget
            | Self::UnusedGroup
            | Self::InvalidPin => &POLICY_CONFLICT,
        }
    }

    /// Whether the state has an automatic fix (syncpack's Fixable class).
    pub(crate) const fn is_fixable(self) -> bool {
        matches!(
            self,
            Self::IsBanned
                | Self::DiffersToHighestOrLowestSemver
                | Self::DiffersToLocal
                | Self::DiffersToSnapTarget
                | Self::DiffersToPin
                | Self::SemverRangeMismatch
                | Self::PinOverridesSemverRange
                | Self::PinOverridesSemverRangeMismatch
                | Self::SameMinorOverridesSemverRange
                | Self::SameMinorOverridesSemverRangeMismatch
        )
    }

    /// What is wrong, as the tail of a sentence about the dependency.
    pub(crate) const fn summary(self) -> &'static str {
        match self {
            Self::IsBanned => "is banned by a version group",
            Self::DiffersToHighestOrLowestSemver => "differs from the version the workspace aligns on",
            Self::DiffersToLocal => "differs from the version of the workspace package",
            Self::DiffersToSnapTarget => "differs from the version of its snap target",
            Self::DiffersToPin => "differs from the pinned version",
            Self::SemverRangeMismatch => "does not use the semver range its semver group requires",
            Self::PinOverridesSemverRange | Self::PinOverridesSemverRangeMismatch => {
                "is pinned, which overrides the range its semver group requires"
            }
            Self::SameMinorOverridesSemverRange | Self::SameMinorOverridesSemverRangeMismatch => {
                "must use a range that stays within one minor, which overrides its semver group"
            }
            Self::NonSemverMismatch => {
                "differs from the other specifiers, and not every one of them is a semver version to align on"
            }
            Self::SameRangeMismatch => "has a range that does not overlap the ranges of all other instances",
            Self::SameMinorMismatch => "is not on the same minor version as the other instances",
            Self::SameMinorHasMajorMismatch => "is not on the same major version as the other instances",
            Self::DependsOnInvalidLocalPackage => "depends on a workspace package whose own version is invalid",
            Self::MatchConflictsWithHighestOrLowestSemver => {
                "uses its semver group's range, which cannot satisfy the version the workspace aligns on"
            }
            Self::MismatchConflictsWithHighestOrLowestSemver => {
                "is outside its semver group's range, which cannot satisfy the version the workspace aligns on"
            }
            Self::MatchConflictsWithSnapTarget => {
                "uses its semver group's range, which cannot satisfy the version of its snap target"
            }
            Self::MismatchConflictsWithSnapTarget => {
                "is outside its semver group's range, which cannot satisfy the version of its snap target"
            }
            Self::MatchConflictsWithLocal => {
                "uses its semver group's range, which cannot satisfy the version of the workspace package"
            }
            Self::MismatchConflictsWithLocal => {
                "is outside its semver group's range, which cannot satisfy the version of the workspace package"
            }
            Self::InvalidLocalVersion => "is not an exact semver version, so dependents cannot be aligned with it",
            Self::RefuseToBanLocal => "is a workspace package; a banned group refuses to ban its own version",
            Self::RefuseToPinLocal => "is a workspace package; a pinned group refuses to pin its own version",
            Self::RefuseToSnapLocal => "is a workspace package; a snapped-to group refuses to change its own version",
            Self::DependsOnMissingSnapTarget => "is snapped to a package that does not declare it",
            Self::UnusedGroup => "is not matched by any dependency",
            Self::InvalidPin => "pins a version that is empty or not a version specifier, so it cannot be applied",
        }
    }
}

/// The state of one instance and the specifier it should have.
#[derive(Debug, Clone)]
pub(crate) struct Verdict {
    pub(crate) state: State,
    /// The specifier the instance should be rewritten to, when the state has one to offer.
    pub(crate) target: Option<Arc<Specifier>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emitted states of the ADR 0009 table with the rule each maps to.
    const TABLE: [(State, &str, &str, bool); 26] = [
        (State::IsBanned, "IsBanned", "banned-dependency", true),
        (
            State::DiffersToHighestOrLowestSemver,
            "DiffersToHighestOrLowestSemver",
            "version-mismatch",
            true,
        ),
        (State::DiffersToLocal, "DiffersToLocal", "version-mismatch", true),
        (
            State::DiffersToSnapTarget,
            "DiffersToSnapTarget",
            "version-mismatch",
            true,
        ),
        (State::DiffersToPin, "DiffersToPin", "version-mismatch", true),
        (
            State::SemverRangeMismatch,
            "SemverRangeMismatch",
            "semver-range-mismatch",
            true,
        ),
        (
            State::PinOverridesSemverRange,
            "PinOverridesSemverRange",
            "policy-conflict",
            true,
        ),
        (
            State::PinOverridesSemverRangeMismatch,
            "PinOverridesSemverRangeMismatch",
            "policy-conflict",
            true,
        ),
        (
            State::SameMinorOverridesSemverRange,
            "SameMinorOverridesSemverRange",
            "policy-conflict",
            true,
        ),
        (
            State::SameMinorOverridesSemverRangeMismatch,
            "SameMinorOverridesSemverRangeMismatch",
            "policy-conflict",
            true,
        ),
        (State::NonSemverMismatch, "NonSemverMismatch", "version-mismatch", false),
        (State::SameRangeMismatch, "SameRangeMismatch", "version-mismatch", false),
        (State::SameMinorMismatch, "SameMinorMismatch", "version-mismatch", false),
        (
            State::SameMinorHasMajorMismatch,
            "SameMinorHasMajorMismatch",
            "version-mismatch",
            false,
        ),
        (
            State::DependsOnInvalidLocalPackage,
            "DependsOnInvalidLocalPackage",
            "invalid-local-version",
            false,
        ),
        (
            State::MatchConflictsWithHighestOrLowestSemver,
            "MatchConflictsWithHighestOrLowestSemver",
            "policy-conflict",
            false,
        ),
        (
            State::MismatchConflictsWithHighestOrLowestSemver,
            "MismatchConflictsWithHighestOrLowestSemver",
            "policy-conflict",
            false,
        ),
        (
            State::MatchConflictsWithSnapTarget,
            "MatchConflictsWithSnapTarget",
            "policy-conflict",
            false,
        ),
        (
            State::MismatchConflictsWithSnapTarget,
            "MismatchConflictsWithSnapTarget",
            "policy-conflict",
            false,
        ),
        (
            State::MatchConflictsWithLocal,
            "MatchConflictsWithLocal",
            "policy-conflict",
            false,
        ),
        (
            State::MismatchConflictsWithLocal,
            "MismatchConflictsWithLocal",
            "policy-conflict",
            false,
        ),
        (
            State::InvalidLocalVersion,
            "InvalidLocalVersion",
            "invalid-local-version",
            false,
        ),
        (State::RefuseToBanLocal, "RefuseToBanLocal", "policy-conflict", false),
        (State::RefuseToPinLocal, "RefuseToPinLocal", "policy-conflict", false),
        (State::RefuseToSnapLocal, "RefuseToSnapLocal", "policy-conflict", false),
        (
            State::DependsOnMissingSnapTarget,
            "DependsOnMissingSnapTarget",
            "policy-conflict",
            false,
        ),
    ];

    #[test]
    fn every_emitted_state_maps_to_the_rule_of_the_adr_table() {
        for (state, name, rule, fixable) in TABLE {
            assert_eq!(state.name(), name);
            assert_eq!(state.rule().id, rule, "{name}");
            assert_eq!(state.is_fixable(), fixable, "{name}");
            assert!(!state.summary().is_empty(), "{name}");
        }
    }

    #[test]
    fn a_non_semver_mismatch_does_not_claim_that_no_specifier_is_semver() {
        // `*` next to `^3.0.0` and `latest` next to `beta` end in the same state; the summary fits both.
        let summary = State::NonSemverMismatch.summary();
        assert!(!summary.contains("none of them"), "{summary}");
        assert!(
            summary.contains("not every one of them is a semver version"),
            "{summary}"
        );
    }

    #[test]
    fn the_skif_only_state_is_a_policy_conflict() {
        assert_eq!(State::UnusedGroup.name(), "UnusedGroup");
        assert_eq!(State::UnusedGroup.rule().id, "policy-conflict");
        assert!(!State::UnusedGroup.is_fixable());
        assert_eq!(State::InvalidPin.name(), "InvalidPin");
        assert_eq!(State::InvalidPin.rule().id, "policy-conflict");
        assert!(!State::InvalidPin.is_fixable());
    }

    #[test]
    fn the_rules_have_distinct_ids_and_sentence_summaries() {
        let metas = [
            &VERSION_MISMATCH,
            &BANNED_DEPENDENCY,
            &SEMVER_RANGE_MISMATCH,
            &POLICY_CONFLICT,
            &INVALID_LOCAL_VERSION,
        ];
        let mut ids: Vec<&str> = metas.iter().map(|meta| meta.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), metas.len());
        assert!(
            metas
                .iter()
                .all(|meta| meta.summary.ends_with('.') && meta.summary.len() > 20)
        );
    }
}
