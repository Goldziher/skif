//! What the policies need to know about a specifier beyond its parsed form: how it ranks, and whether it aliases.

use skif_specifier::Specifier;
use skif_specifier::semver_range::SemverRange;

/// The lowest version `specifier` admits, for ranking.
///
/// A shorthand such as `^18` or `18.3` names the first release of that line: `18.0.0` and `18.3.0`. The version the
/// parser derives for ordering (`18.999999.999999`) puts such a range above `^18.3.1`, which would make the vaguer
/// specifier the one everything else aligns with. `None` when the specifier names no version.
pub(super) fn floor_version(specifier: &Specifier) -> Option<impl Ord + Clone + use<>> {
    let floor = specifier.get_semver_number().and_then(|number| {
        let (core, suffix) = number.split_at(number.find(['-', '+']).unwrap_or(number.len()));
        let mut parts: Vec<&str> = core.split('.').collect();
        while parts.len() < 3 {
            parts.push("0");
        }
        Specifier::new_node_version(&format!("{}{suffix}", parts.join(".")))
    });
    floor.or_else(|| specifier.get_node_version())
}

/// Whether rewriting `from` to `to` is a version alignment that `--fix` may apply on its own (ADR 0009).
///
/// Both must be plain versions or `^`/`~` ranges of the same shape (`1.2.3` stays exact, `^1.2` stays `^` with a
/// major and a minor), and an identical specifier is always safe. Otherwise neither may carry a prerelease or build
/// suffix, `to` must not be lower than `from`, and it must stay inside what semver calls compatible with `from`: the
/// same major, the same minor under `0.x`, the same patch under `0.0.x`. A `~` range that names a minor also keeps
/// that minor. Anything else (a cross-major move, a downgrade, a change of range, an alias, a
/// complex range) stays unsafe.
pub(super) fn is_compatible_upgrade(from: &Specifier, to: &Specifier) -> bool {
    let (Some(from_shape), Some(to_shape)) = (shape(from), shape(to)) else {
        return false;
    };
    if from_shape != to_shape {
        return false;
    }
    if from.get_raw() == to.get_raw() {
        return true;
    }
    let (Some(old), Some(new)) = (release(from), release(to)) else {
        return false;
    };
    let semver_window = match old {
        (0, 0, _) => new == old,
        (0, minor, _) => new.0 == 0 && new.1 == minor,
        (major, _, _) => new.0 == major,
    };
    let names_minor = from.get_semver_number().is_some_and(|number| number.contains('.'));
    let shape_window = match from_shape.1 {
        '~' if names_minor => (new.0, new.1) == (old.0, old.1),
        _ => true,
    };
    semver_window && shape_window && new >= old
}

/// The kind of specifier and its range operator, for the plain versions and `^`/`~` ranges only.
fn shape(specifier: &Specifier) -> Option<(std::mem::Discriminant<Specifier>, char)> {
    let plain = matches!(
        specifier,
        Specifier::Exact(_)
            | Specifier::Major(_)
            | Specifier::Minor(_)
            | Specifier::Range(_)
            | Specifier::RangeMajor(_)
            | Specifier::RangeMinor(_)
    );
    let operator = match specifier.get_semver_range()? {
        SemverRange::Exact => '=',
        SemverRange::Minor => '^',
        SemverRange::Patch => '~',
        _ => return None,
    };
    plain.then(|| (std::mem::discriminant(specifier), operator))
}

/// The release `specifier` names as major, minor and patch, missing parts being zero; `None` with a prerelease or
/// build suffix.
fn release(specifier: &Specifier) -> Option<(u64, u64, u64)> {
    let number = specifier.get_semver_number()?;
    if number.contains(['-', '+']) {
        return None;
    }
    let mut parts = number.split('.').map(digits);
    let mut next = || parts.next().unwrap_or(Some(0));
    let release = (next()?, next()?, next()?);
    parts.next().is_none().then_some(release)
}

/// `part` as a decimal number, as `str::parse::<u64>` reads one without a sign (`release` has rejected `+` and `-`):
/// `None` when it is empty, holds another character or overflows. A hand-written loop: the generic parser was most of
/// the time spent rating the fixes of a large workspace.
fn digits(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    part.bytes().try_fold(0_u64, |value, byte| {
        let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
        value.checked_mul(10)?.checked_add(u64::from(digit))
    })
}

/// The major version `specifier` names, for telling release lines apart.
pub(super) fn major(specifier: &Specifier) -> Option<u64> {
    specifier.get_node_version().map(|version| version.major())
}

/// The package an `npm:` alias installs when that is not the dependency `name` itself.
pub(super) fn alias_target<'a>(specifier: &'a Specifier, name: &str) -> Option<&'a str> {
    match specifier {
        Specifier::Alias(alias) if alias.name != name => Some(alias.name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_floor(first: &str, second: &str) -> bool {
        floor_version(&Specifier::create(first)) == floor_version(&Specifier::create(second))
    }

    #[test]
    fn shorthands_rank_at_their_first_release() {
        assert!(same_floor("^18", "18.0.0"));
        assert!(same_floor("18", "18.0.0"));
        assert!(same_floor("~18.3", "18.3.0"));
        assert!(same_floor("^18.3.1", "18.3.1"));
        assert!(same_floor("^1.0.0-beta.1", "1.0.0-beta.1"));
        assert!(floor_version(&Specifier::create("beta")).is_none());
    }

    #[test]
    fn wildcard_specifiers_rank_as_syncpack_ranks_them() {
        // `18.x` is not a specifier of its own (only a part of a complex range), so it has no version to rank by.
        assert!(floor_version(&Specifier::create("18.x")).is_none());
        // `*` admits every version and ranks above any concrete one, as in syncpack.
        let precise = floor_version(&Specifier::create("^18.3.1"));
        assert!(floor_version(&Specifier::create("*")) > precise);
    }

    #[test]
    fn an_alias_to_another_package_names_its_target() {
        let target = |raw: &str| alias_target(&Specifier::create(raw), "foo").map(str::to_owned);
        assert_eq!(target("npm:bar@^1.0.0").as_deref(), Some("bar"));
        assert_eq!(target("npm:@scope/bar@^1.0.0").as_deref(), Some("@scope/bar"));
        assert_eq!(target("npm:foo@^1.0.0"), None);
        assert_eq!(target("^1.0.0"), None);
    }

    #[test]
    fn only_compatible_upgrades_of_the_same_shape_are_safe_alignments() {
        let safe = |from: &str, to: &str| is_compatible_upgrade(&Specifier::create(from), &Specifier::create(to));
        let cases = [
            ("1.0.0", "1.0.5", true),
            ("1.0.0", "1.3.0", true),
            ("1.0.0", "2.0.0", false),
            ("1.3.0", "1.3.0", true),
            ("^1.0.0", "^1.3.0", true),
            ("^1.0.0", "^2.0.0", false),
            ("~1.2.0", "~1.2.5", true),
            ("~1.2.0", "~1.3.0", false),
            ("~1", "~1", true),
            ("~1", "~1.4", false),
            ("^1", "^1.4", false),
            ("^1.2", "^1.4", true),
            ("0.4.1", "0.4.9", true),
            ("0.4.1", "0.5.0", false),
            ("^0.4.1", "^0.4.9", true),
            ("^0.4.1", "^0.5.0", false),
            ("^0.0.3", "^0.0.4", false),
            ("0.0.3", "0.0.4", false),
            ("6.0.0", "7.0.0", false),
            ("1.3.0", "1.0.0", false),
            ("1.0.5", "1.0.0", false),
            ("^1.3.0", "^1.0.0", false),
            ("^1.0.0", "1.3.0", false),
            ("1.0.0", "^1.3.0", false),
            ("~1.0.0", "^1.0.0", false),
            (">=1.0.0", ">=1.3.0", false),
            ("1.0.0", "1.0.1-beta.1", false),
            ("1.0.0-beta.1", "1.0.0", false),
            ("1.0.0-beta.1", "1.0.0-beta.2", false),
            ("1.0.0-beta.1", "1.0.0-beta.1", true),
            ("npm:a@1.0.0", "npm:a@1.3.0", false),
            ("npm:a@1.0.0", "npm:a@1.0.0", false),
            ("latest", "1.3.0", false),
            ("latest", "latest", false),
            ("1.0.0", "workspace:1.3.0", false),
            ("workspace:*", "workspace:*", false),
        ];
        for (from, to, expected) in cases {
            assert_eq!(safe(from, to), expected, "{from} -> {to}");
        }
    }

    #[test]
    fn digits_read_a_number_as_the_standard_parser_does() {
        let parts = [
            "0",
            "7",
            "007",
            "18",
            "",
            "1x",
            "x",
            " 1",
            "18446744073709551615",
            "18446744073709551616",
            "99999999999999999999",
            "\u{663}",
        ];
        for part in parts {
            assert_eq!(digits(part), part.parse::<u64>().ok(), "{part}");
        }
    }

    #[test]
    fn the_major_comes_from_the_version_number() {
        assert_eq!(major(&Specifier::create("^18")), Some(18));
        assert_eq!(major(&Specifier::create("0.4.2")), Some(0));
        assert_eq!(major(&Specifier::create("beta")), None);
    }

    #[test]
    fn a_shorthand_ranks_below_a_full_version_of_its_line() {
        let vague = floor_version(&Specifier::create("^18"));
        let precise = floor_version(&Specifier::create("^18.3.1"));
        assert!(vague < precise);
        assert!(floor_version(&Specifier::create("^19")) > precise);
    }
}
