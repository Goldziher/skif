//! `--merge`: combining the generated sections with an existing `skif.toml`.

use toml::{Table, Value};

use crate::MigrateError;
use crate::doc::Doc;
use crate::state::validate;

/// The result of merging into an existing file.
#[derive(Debug, Clone)]
pub struct Merged {
    /// The merged `skif.toml`.
    pub toml: String,
    /// The keys where the existing value was kept over the generated one, as dotted paths.
    pub kept_existing: Vec<String>,
    /// Whether the comments of the existing file are still in `toml`. They are unless both files defined the same
    /// table, which forces a merge of the parsed values.
    pub comments_preserved: bool,
}

pub(crate) fn merge_into(doc: &Doc, generated: &str, existing: &str) -> Result<Merged, MigrateError> {
    if existing.trim().is_empty() {
        return Ok(Merged {
            toml: generated.to_owned(),
            kept_existing: Vec::new(),
            comments_preserved: true,
        });
    }
    let current: Table = existing
        .parse()
        .map_err(|error: toml::de::Error| MigrateError::Existing(error.to_string()))?;
    if let Some(toml) = append(doc, existing, &current)?
        && validate(&toml).is_ok()
    {
        return Ok(Merged {
            toml,
            kept_existing: Vec::new(),
            comments_preserved: true,
        });
    }
    let added: Table = generated
        .parse()
        .map_err(|error: toml::de::Error| MigrateError::Invalid(error.to_string()))?;
    let mut merged = current.clone();
    let mut kept_existing = Vec::new();
    merge_table(&mut merged, &added, "", &mut kept_existing);
    if merged == current {
        return Ok(Merged {
            toml: existing.to_owned(),
            kept_existing,
            comments_preserved: true,
        });
    }
    let toml = toml::to_string(&merged).map_err(|error| MigrateError::Invalid(error.to_string()))?;
    validate(&toml)?;
    Ok(Merged {
        toml,
        kept_existing,
        comments_preserved: false,
    })
}

/// The existing text followed by the generated sections it has no counterpart of. `None` when a generated table
/// already exists, which only a merge of values can resolve.
fn append(doc: &Doc, existing: &str, current: &Table) -> Result<Option<String>, MigrateError> {
    let mut added = Vec::new();
    for section in doc.ordered() {
        let text = section.render();
        let parsed: Table = text
            .parse()
            .map_err(|error: toml::de::Error| MigrateError::Invalid(error.to_string()))?;
        if section.array {
            let key = &section.path[0];
            let element = parsed
                .get(key)
                .and_then(Value::as_array)
                .and_then(|items| items.first());
            match (current.get(key), element) {
                (None, _) => added.push(text),
                (Some(Value::Array(items)), Some(element)) => {
                    if !items.contains(element) {
                        added.push(text);
                    }
                }
                _ => return Ok(None),
            }
        } else if exists(current, &section.path) {
            return Ok(None);
        } else {
            added.push(text);
        }
    }
    if added.is_empty() {
        return Ok(Some(existing.to_owned()));
    }
    let mut out = existing.trim_end().to_owned();
    out.push_str("\n\n# Added by `skif migrate`.\n");
    out.push_str(&added.join("\n"));
    Ok(Some(out))
}

fn exists(table: &Table, path: &[String]) -> bool {
    let Some((head, rest)) = path.split_first() else {
        return true;
    };
    match table.get(head) {
        None => false,
        Some(Value::Table(inner)) => exists(inner, rest),
        Some(_) => true,
    }
}

/// Adds `added` to `base`: tables merge, lists unite, and the existing value wins over a different one.
fn merge_table(base: &mut Table, added: &Table, path: &str, kept: &mut Vec<String>) {
    for (key, value) in added {
        let here = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let Some(current) = base.get_mut(key) else {
            base.insert(key.clone(), value.clone());
            continue;
        };
        match (current, value) {
            (Value::Table(inner), Value::Table(more)) => merge_table(inner, more, &here, kept),
            (Value::Array(items), Value::Array(more)) => {
                for item in more {
                    if !items.contains(item) {
                        items.push(item.clone());
                    }
                }
            }
            (current, value) if current == value => {}
            _ => kept.push(here),
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
        assert!(!merged.comments_preserved);
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
