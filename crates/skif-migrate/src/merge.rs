//! `--merge`: combining the generated sections with an existing `skif.toml`.
//!
//! The existing file is edited in place with `toml_edit`, so its comments, order and formatting survive: missing
//! tables and keys are added, arrays and arrays of tables gain the entries they lack, and a key both files set to
//! different values keeps the existing value (and is reported).

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::MigrateError;
use crate::doc::Doc;

/// The result of merging into an existing file.
#[derive(Debug, Clone)]
pub struct Merged {
    /// The merged `skif.toml`.
    pub toml: String,
    /// The keys where the existing value was kept over the generated one, as dotted paths.
    pub kept_existing: Vec<String>,
    /// Whether the comments of the existing file are still in `toml`; always true since the file is edited in place.
    pub comments_preserved: bool,
}

pub(crate) fn merge_into(_doc: &Doc, generated: &str, existing: &str) -> Result<Merged, MigrateError> {
    if existing.trim().is_empty() {
        return Ok(Merged {
            toml: generated.to_owned(),
            kept_existing: Vec::new(),
            comments_preserved: true,
        });
    }
    let mut current: DocumentMut = existing
        .parse()
        .map_err(|error: toml_edit::TomlError| MigrateError::Existing(error.to_string()))?;
    let added: DocumentMut = generated
        .parse()
        .map_err(|error: toml_edit::TomlError| MigrateError::Invalid(error.to_string()))?;
    let mut kept_existing = Vec::new();
    merge_table(current.as_table_mut(), added.as_table(), "", &mut kept_existing);
    Ok(Merged {
        toml: current.to_string(),
        kept_existing,
        comments_preserved: true,
    })
}

fn same(left: &impl std::fmt::Display, right: &impl std::fmt::Display) -> bool {
    left.to_string().trim() == right.to_string().trim()
}

fn merge_table(into: &mut Table, from: &Table, prefix: &str, kept: &mut Vec<String>) {
    for (key, item) in from {
        let path = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        match (into.get_mut(key), item) {
            (None, _) => {
                into.insert(key, item.clone());
            }
            (Some(Item::Table(existing)), Item::Table(added)) => merge_table(existing, added, &path, kept),
            (Some(Item::ArrayOfTables(existing)), Item::ArrayOfTables(added)) => {
                for table in added {
                    if !existing.iter().any(|present| same(present, table)) {
                        existing.push(table.clone());
                    }
                }
            }
            (Some(Item::Value(Value::Array(existing))), Item::Value(Value::Array(added))) => {
                for value in added {
                    if !existing.iter().any(|present| same(present, value)) {
                        existing.push_formatted(value.clone());
                    }
                }
            }
            (Some(existing), added) => {
                if !same(existing, added) {
                    kept.push(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Val;

    fn doc() -> Doc {
        let mut doc = Doc::default();
        doc.table(&["lint"]).set("fail-on-warnings", Val::Bool(true));
        doc.element(&["ignore"], vec![])
            .set("dependencies", Val::strs(["react"]));
        doc
    }

    #[test]
    fn new_sections_are_appended_after_the_existing_text() {
        let generated = doc().render(&[]);
        let existing = "# mine\n[workspace]\nignore-packages = [\"a\"]\n";
        let merged = merge_into(&doc(), &generated, existing).unwrap();
        assert!(merged.comments_preserved);
        assert!(merged.toml.starts_with("# mine\n[workspace]\n"));
        assert!(merged.toml.contains("[lint]\nfail-on-warnings = true"));
        assert!(merged.toml.contains("[[ignore]]\ndependencies = [\"react\"]"));
        let both = merge_into(&doc(), &generated, "# keep\n[lint] # inline\nmax-level = \"warn\"\n").unwrap();
        assert!(
            both.toml.contains("# keep") && both.toml.contains("# inline"),
            "{}",
            both.toml
        );
        assert!(both.toml.contains("fail-on-warnings = true"), "{}", both.toml);
    }

    #[test]
    fn merging_the_output_into_itself_changes_nothing() {
        let generated = doc().render(&[]);
        let merged = merge_into(&doc(), &generated, &generated).unwrap();
        assert_eq!(merged.toml, generated);
    }

    #[test]
    fn an_existing_value_wins_over_a_different_one() {
        let generated = doc().render(&[]);
        let existing = "[lint]\nfail-on-warnings = false\n";
        let merged = merge_into(&doc(), &generated, existing).unwrap();
        assert_eq!(merged.kept_existing, ["lint.fail-on-warnings"]);
        assert!(merged.toml.contains("fail-on-warnings = false"));
        assert!(merged.toml.contains("react"));
    }

    #[test]
    fn invalid_existing_text_is_an_error() {
        let result = merge_into(&doc(), "", "[lint");
        assert!(matches!(result, Err(MigrateError::Existing(_))));
    }
}
