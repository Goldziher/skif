//! The two static tables of the translation, kept as data files so a test can check them against the rule list.

const RENAMES: &str = include_str!("rule-renames.txt");
const KNIP_TYPES: &str = include_str!("knip-types.txt");
const TOOL_RULES: &str = include_str!("tool-rules.txt");

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

/// The skif rules syncpack 15 reports under `condition`: `always`, `catalog-policy` or `sort-az`.
pub(crate) fn syncpack_rules(condition: &str) -> Vec<&'static str> {
    rows(TOOL_RULES)
        .filter(|row| row[1] == "syncpack" && row[2] == condition)
        .map(|row| row[0])
        .collect()
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

    fn covered_rules() -> Vec<&'static str> {
        let mut covered: Vec<&'static str> = rows(RENAMES).map(|row| row[1]).collect();
        covered.extend(knip_types().into_iter().flat_map(|knip| knip.rules));
        covered.extend(rows(TOOL_RULES).filter(|row| row[1] == "syncpack").map(|row| row[0]));
        covered
    }

    #[test]
    fn every_rule_is_covered_by_a_tool_or_declared_uncovered() {
        let covered = covered_rules();
        let uncovered: Vec<&str> = rows(TOOL_RULES).filter(|row| row[1] == "none").map(|row| row[0]).collect();
        for id in rule_ids() {
            assert!(
                covered.contains(&id.as_str()) != uncovered.contains(&id.as_str()),
                "{id} must be in exactly one of the tool tables or the `none` rows"
            );
        }
        for id in &uncovered {
            assert!(rows(TOOL_RULES).any(|row| row[0] == *id && row.len() > 3), "{id} has no reason");
        }
    }

    #[test]
    fn syncpack_conditions_are_known() {
        for row in rows(TOOL_RULES).filter(|row| row[1] == "syncpack") {
            assert!(["always", "catalog-policy", "sort-az"].contains(&row[2]), "{}", row[0]);
        }
        assert!(syncpack_rules("catalog-policy").contains(&"catalog-not-used"));
        assert!(!syncpack_rules("always").contains(&"catalog-not-used"));
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
