// Added for skif (not in syncpack): property tests over the specifier parser; see ATTRIBUTIONS.md.
use {super::hardening_test::exercise, crate::Specifier, proptest::prelude::*, std::cmp::Ordering};

fn number() -> impl Strategy<Value = String> {
    prop_oneof![
        "[0-9]{1,3}",
        "[0-9]{15,25}",
        "0",
        "x",
        "X",
        "\\*",
        "900719925474099",
        "900719925474100",
        "18446744073709551615",
        "18446744073709551616",
    ]
}

fn operator() -> impl Strategy<Value = String> {
    prop_oneof!["", "\\^", "~", ">", ">=", "<", "<=", "=", "v"]
}

fn version() -> impl Strategy<Value = String> {
    (
        operator(),
        number(),
        proptest::option::of(number()),
        proptest::option::of(number()),
        proptest::option::of("[a-z0-9.\\-]{1,8}"),
        proptest::option::of("[a-z0-9.]{1,6}"),
    )
        .prop_map(|(op, major, minor, patch, pre, build)| {
            let mut out = format!("{op}{major}");
            for part in [minor, patch].into_iter().flatten() {
                out.push('.');
                out.push_str(&part);
            }
            if let Some(pre) = pre {
                out.push('-');
                out.push_str(&pre);
            }
            if let Some(build) = build {
                out.push('+');
                out.push_str(&build);
            }
            out
        })
}

fn separator() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just(" "), Just(" || "), Just("||"), Just(" - "), Just("-"), Just("  ")]
}

fn prefix() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just(""),
        Just(""),
        Just(""),
        Just("workspace:"),
        Just("catalog:"),
        Just("npm:foo@"),
        Just("npm:@scope/pkg@"),
        Just("npm:"),
        Just("link:"),
        Just("file:"),
        Just("github:user/repo#"),
        Just("github:user/repo#semver:"),
        Just("git+https://host/repo.git#"),
        Just("git@host:user/repo.git#"),
        Just("https://host/pkg#"),
    ]
}

fn semverish() -> impl Strategy<Value = String> {
    (
        prefix(),
        proptest::collection::vec(version(), 1..4),
        separator(),
        "[\\^~<>=*xX|. \\-+a-z0-9:@/#]{0,12}",
    )
        .prop_map(|(prefix, versions, separator, noise)| format!("{prefix}{}{noise}", versions.join(separator)))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn arbitrary_strings_never_panic(a in any::<String>(), b in any::<String>()) {
        exercise(&Specifier::new(&a), &Specifier::new(&b));
    }

    #[test]
    fn semverish_strings_never_panic(a in semverish(), b in semverish()) {
        exercise(&Specifier::new(&a), &Specifier::new(&b));
    }

    #[test]
    fn create_preserves_the_raw_value(a in semverish()) {
        let created = Specifier::create(&a);
        prop_assert_eq!(created.get_raw(), a.as_str());
    }

    #[test]
    fn cmp_is_equal_exactly_when_eq(a in semverish(), b in semverish()) {
        let (left, right) = (Specifier::new(&a), Specifier::new(&b));
        prop_assert_eq!(left.cmp(&right) == Ordering::Equal, left == right);
        prop_assert_eq!(left.cmp(&right), right.cmp(&left).reverse());
        prop_assert_eq!(left.partial_cmp(&right), Some(left.cmp(&right)));
    }

    #[test]
    fn cmp_is_equal_exactly_when_eq_for_arbitrary_strings(a in any::<String>(), b in any::<String>()) {
        let (left, right) = (Specifier::new(&a), Specifier::new(&b));
        prop_assert_eq!(left.cmp(&right) == Ordering::Equal, left == right);
    }
}
