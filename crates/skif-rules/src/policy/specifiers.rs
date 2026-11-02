//! What the policies need to know about a specifier beyond its parsed form: how it ranks, and whether it aliases.

use skif_specifier::Specifier;

/// The lowest version `specifier` admits, for ranking.
///
/// A shorthand such as `^18` or `18.3` names the first release of that line: `18.0.0` and `18.3.0`. The version the
/// parser derives for ordering (`18.999999.999999`) puts such a range above `^18.3.1`, which would make the vaguer
/// specifier the one everything else aligns with. `None` when the specifier names no version.
pub(super) fn floor_version(specifier: &Specifier) -> Option<impl Ord + Clone> {
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
    fn an_alias_to_another_package_names_its_target() {
        let target = |raw: &str| alias_target(&Specifier::create(raw), "foo").map(str::to_owned);
        assert_eq!(target("npm:bar@^1.0.0").as_deref(), Some("bar"));
        assert_eq!(target("npm:@scope/bar@^1.0.0").as_deref(), Some("@scope/bar"));
        assert_eq!(target("npm:foo@^1.0.0"), None);
        assert_eq!(target("^1.0.0"), None);
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
