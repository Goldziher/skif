//! The two static tables of the translation, kept as data files so a test can check them against the rule list.

const RENAMES: &str = include_str!("rule-renames.txt");
const KNIP_TYPES: &str = include_str!("knip-types.txt");

fn rows(text: &'static str) -> impl Iterator<Item = Vec<&'static str>> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| line.split_whitespace().collect())
}

/// The skif rule id that replaces the sherif rule `name`.
pub(crate) fn sherif_rule(name: &str) -> Option<&'static str> {
    rows(RENAMES).find(|row| row[0] == name).map(|row| row[1])
}

/// The skif rule ids a sherif rule maps to, for the rules that sherif ran.
pub(crate) fn sherif_rules() -> Vec<&'static str> {
    rows(RENAMES).map(|row| row[1]).collect()
}

/// A knip issue type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnipType {
    pub name: &'static str,
    /// The skif rules that report the same findings; empty when skif has none.
    pub rules: Vec<&'static str>,
    /// Whether knip reports the type when nothing narrows the run.
    pub default: bool,
}

/// Every knip issue type, in the order of the table.
pub(crate) fn knip_types() -> Vec<KnipType> {
    rows(KNIP_TYPES)
        .map(|row| KnipType {
            name: row[0],
            rules: if row[1] == "-" {
                Vec::new()
            } else {
                row[1].split(',').collect()
            },
            default: row[2] == "yes",
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_ids() -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rule-ids.txt");
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn every_target_rule_exists() {
        let known = rule_ids();
        let targets = rows(RENAMES)
            .map(|row| row[1])
            .chain(knip_types().into_iter().flat_map(|knip| knip.rules));
        for target in targets {
            assert!(known.iter().any(|id| id == target), "{target} is not a skif rule");
        }
    }

    #[test]
    fn renames_agree_with_the_retired_ids() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skif-rules/tests/retired-rule-ids.txt");
        let text = std::fs::read_to_string(path).unwrap();
        let retired: Vec<(&str, &str)> = text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .filter_map(|line| line.split_once(' '))
            .collect();
        for (old, new) in &retired {
            if let Some(mine) = sherif_rule(old) {
                assert_eq!(mine, *new, "{old}");
            }
        }
        for row in rows(RENAMES).filter(|row| row[0] != row[1]) {
            assert!(retired.contains(&(row[0], row[1])), "{} is not a retired id", row[0]);
        }
    }

    #[test]
    fn sherif_has_ten_rules() {
        assert_eq!(sherif_rules().len(), 10);
        assert_eq!(sherif_rule("unordered-dependencies"), Some("unsorted-dependencies"));
        assert_eq!(sherif_rule("nope"), None);
    }

    #[test]
    fn knip_defaults_exclude_the_opt_in_types() {
        let off: Vec<&str> = knip_types()
            .iter()
            .filter(|knip| !knip.default)
            .map(|knip| knip.name)
            .collect();
        assert_eq!(off, ["nsExports", "nsTypes", "cycles"]);
    }
}
