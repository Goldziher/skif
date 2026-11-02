use {
    crate::{Specifier, semver_range::SemverRange, update_target::UpdateTarget},
    std::{
        hash::{Hash, Hasher},
        sync::Arc,
    },
};

const ALL_RANGES: [SemverRange; 8] = [
    SemverRange::Any,
    SemverRange::Minor,
    SemverRange::Exact,
    SemverRange::Gt,
    SemverRange::Gte,
    SemverRange::Lt,
    SemverRange::Lte,
    SemverRange::Patch,
];

/// Call every public getter and method of `a`, alone and against `b`. Used to
/// prove none of them panic, whatever the input.
pub fn exercise(a: &Arc<Specifier>, b: &Arc<Specifier>) {
    let _ = a.get_config_identifier();
    let _ = a.get_raw();
    let _ = a.get_semver_number();
    let _ = a.get_node_version();
    let _ = a.get_node_range();
    let _ = a.get_semver_range();
    let _ = a.is_workspace_protocol();
    let _ = a.is_link();
    let _ = a.is_catalog();
    let _ = format!("{a:?}");

    for range in &ALL_RANGES {
        let _ = a.with_range(range);
    }

    let versions = [
        nodejs_semver::Version::new(1, 2, 3, Vec::new(), Vec::new()),
        nodejs_semver::Version::new(crate::HUGE, crate::HUGE, crate::HUGE, Vec::new(), Vec::new()),
        nodejs_semver::Version::new(0, 0, 0, Vec::new(), Vec::new()),
    ];
    for version in &versions {
        let _ = a.with_node_version(version);
    }
    if let Some(version) = Specifier::new_node_version("2.0.0-beta.1+build.5") {
        let _ = a.with_node_version(&version);
    }

    let _ = a.has_same_release_channel_as(b);
    let _ = a.has_same_version_number_as(b);
    for target in [UpdateTarget::Latest, UpdateTarget::Minor, UpdateTarget::Patch] {
        let _ = a.is_eligible_update_for(b, &target);
    }
    let _ = a.satisfies_all(&[Arc::clone(a), Arc::clone(b)]);
    let _ = a.satisfies_all(&[]);

    let _ = a.cmp(b);
    let _ = a.partial_cmp(b);
    let _ = a == b;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    a.hash(&mut hasher);
    let _ = hasher.finish();
}

fn assert_unsupported(value: &str) {
    let specifier = Specifier::new(value);
    assert!(
        matches!(&*specifier, Specifier::Unsupported(raw) if raw == value),
        "expected Unsupported for {value:?}, got {specifier:?}"
    );
    exercise(&specifier, &specifier);
}

fn assert_complex(value: &str) {
    let specifier = Specifier::new(value);
    assert!(
        matches!(&*specifier, Specifier::ComplexSemver(complex) if complex.raw == value),
        "expected ComplexSemver for {value:?}, got {specifier:?}"
    );
    assert!(specifier.get_node_range().is_some(), "{value:?} lost its node range");
    exercise(&specifier, &specifier);
}

// =============================================================================
// Out-of-range numbers are Unsupported, never a panic
// =============================================================================

#[test]
fn out_of_range_numbers_are_unsupported() {
    let cases = [
        "99999999999999999999",
        "1.99999999999999999999",
        "99999999999999999999.1",
        "900719925474100",
        "1.900719925474100",
        "^99999999999999999999",
        "~99999999999999999999",
        ">99999999999999999999",
        ">=99999999999999999999",
        "<99999999999999999999",
        "<=99999999999999999999",
        "^1.99999999999999999999",
        "~1.99999999999999999999",
        "^99999999999999999999.1",
        "1.2.99999999999999999999",
        "^1.2.99999999999999999999",
        "99999999999999999999.0.0",
        "18446744073709551615",
        "18446744073709551616",
        "1.18446744073709551615",
    ];
    for value in cases {
        assert_unsupported(value);
    }
}

#[test]
fn out_of_range_numbers_nested_in_other_variants_do_not_panic() {
    let cases = [
        "workspace:99999999999999999999",
        "workspace:^99999999999999999999",
        "workspace:~1.99999999999999999999",
        "npm:foo@99999999999999999999",
        "npm:foo@^1.99999999999999999999",
        "github:user/repo#99999999999999999999",
        "github:user/repo#semver:^99999999999999999999",
        "git+https://host/repo.git#~1.99999999999999999999",
    ];
    for value in cases {
        let specifier = Specifier::new(value);
        assert!(
            specifier.get_node_version().is_none(),
            "{value:?} must not expose a version, got {specifier:?}"
        );
        exercise(&specifier, &specifier);
    }
}

#[test]
fn largest_safe_numbers_still_parse_and_do_not_panic() {
    let max = "900719925474099";
    let major = Specifier::new(max);
    assert!(matches!(&*major, Specifier::Major(_)), "got {major:?}");
    assert!(major.get_node_version().is_some());
    exercise(&major, &major);

    let minor = Specifier::new(&format!("1.{max}"));
    assert!(matches!(&*minor, Specifier::Minor(_)), "got {minor:?}");
    exercise(&minor, &minor);

    let range_major = Specifier::new(&format!("^{max}"));
    assert!(matches!(&*range_major, Specifier::RangeMajor(_)), "got {range_major:?}");
    exercise(&range_major, &range_major);

    let range_minor = Specifier::new(&format!("~1.{max}"));
    assert!(matches!(&*range_minor, Specifier::RangeMinor(_)), "got {range_minor:?}");
    exercise(&range_minor, &range_minor);
}

#[test]
fn valid_shorthands_still_have_versions_and_ranges() {
    for value in ["1", "1.2", "^1", "~1", ">=1", "^1.2", "<=1.2"] {
        let specifier = Specifier::new(value);
        assert!(specifier.get_node_version().is_some(), "{value:?} has no version");
        assert!(specifier.get_node_range().is_some(), "{value:?} has no range");
        exercise(&specifier, &specifier);
    }
}

// =============================================================================
// Complex ranges and wildcards are classified explicitly
// =============================================================================

#[test]
fn unions_hyphen_ranges_and_conjunctions_are_complex() {
    for value in [
        "1.0.0 || 2.0.0",
        "^1.0.0 || ^2.0.0",
        "1.0.0 - 2.0.0",
        "1.0.0 - 2.0.0 || 3.0.0",
        ">=1.0.0 <2.0.0",
        "1.2.3-beta.1 || 2.0.0",
        "1.0.0 || 2.0.0 || 3.0.0",
        "1.x || 2.x",
        "1.2.* || >=3.0.0",
    ] {
        assert_complex(value);
    }
}

#[test]
fn unspaced_unions_are_never_misclassified() {
    for value in ["1.0.0||2.0.0", "^1.0.0||^2.0.0"] {
        let specifier = Specifier::new(value);
        assert!(
            matches!(&*specifier, Specifier::ComplexSemver(_) | Specifier::Unsupported(_)),
            "{value:?} was classified as {specifier:?}"
        );
        assert_eq!(specifier.get_raw(), value);
    }
}

#[test]
fn x_ranges_on_their_own_are_unsupported() {
    for value in ["1.x", "1.X", "1.2.x", "1.2.*", "1.*", "^1.x", "~1.2.x", ">=1.2.x"] {
        assert_unsupported(value);
    }
}

#[test]
fn malformed_unions_are_unsupported() {
    for value in [
        "1.0.0 || foo",
        "1.0.0 ||",
        "|| 1.0.0",
        "1.0.0 || || 2.0.0",
        "1.0.0 - ",
        "1.0.0 2.0.0 nonsense",
        ">= 1.0.0",
    ] {
        let specifier = Specifier::new(value);
        assert!(
            matches!(&*specifier, Specifier::Unsupported(raw) if raw == value),
            "{value:?} was classified as {specifier:?}"
        );
    }
}

#[test]
fn wildcards_only_match_when_the_whole_value_is_a_wildcard() {
    assert!(matches!(&*Specifier::new("*"), Specifier::Latest(_)));
    assert!(matches!(&*Specifier::new("x"), Specifier::Latest(_)));
    assert!(matches!(&*Specifier::new("X"), Specifier::Latest(_)));
    assert!(matches!(&*Specifier::new("xyz"), Specifier::Tag(_)));
    assert_unsupported("*foo");
    assert_unsupported("**");
    assert_unsupported("*.*");
}

#[test]
fn every_classification_is_stable_for_its_raw_value() {
    for value in [
        "1.2.3",
        "=1.2.3",
        "1",
        "1.2",
        "^1",
        "~1.2",
        "1.0.0 - 2.0.0",
        "1.x",
        "}wat{",
        "99999999999999999999",
    ] {
        assert_eq!(Specifier::new(value).get_raw(), value);
    }
}

#[test]
fn every_identifier_a_specifier_reports_is_listed() {
    let samples = [
        "npm:lodash@^4.17.21",
        "catalog:",
        "^1.2.3 || ^2",
        "1.2.3",
        "file:../a",
        "link:../a",
        "github:user/repo",
        "latest",
        "1",
        "1.2",
        "",
        "^1.2.3",
        "^1",
        "^1.2",
        "beta",
        "!!!",
        "https://example.com/a.tgz",
        "workspace:*",
    ];
    for raw in samples {
        let identifier = Specifier::new(raw).get_config_identifier();
        assert!(
            crate::CONFIG_IDENTIFIERS.contains(&identifier),
            "{raw:?} -> {identifier:?}"
        );
    }
    let mut unique = crate::CONFIG_IDENTIFIERS.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), crate::CONFIG_IDENTIFIERS.len());
}
