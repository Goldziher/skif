//! Fixes: structured operations, text edits and their in-memory application.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How risky applying a fix is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum FixSafety {
    /// Never changes behavior.
    Safe,
    /// May change behavior; requires opt-in.
    Unsafe,
}

/// A titled, ordered group of operations that repairs one diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Fix {
    /// Short description shown to the user.
    pub title: String,
    /// Risk level.
    pub safety: FixSafety,
    /// Operations, applied in order.
    pub ops: Vec<FixOp>,
}

impl Fix {
    /// A fix titled `title` that performs `ops` in order.
    #[must_use]
    pub fn new(title: impl Into<String>, safety: FixSafety, ops: Vec<FixOp>) -> Self {
        Self {
            title: title.into(),
            safety,
            ops,
        }
    }
}

impl JsonAction {
    /// Inserts `key` with `value` into the object node at `position`.
    #[must_use]
    pub const fn insert(key: String, value: serde_json::Value, position: InsertPosition) -> Self {
        Self::Insert { key, value, position }
    }
}

/// One step of a structured path into a JSON or YAML document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum PathSeg {
    /// Object or mapping key.
    Key(String),
    /// Array or sequence index.
    Index(usize),
}

/// Where a new key is placed inside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum InsertPosition {
    /// Before all existing keys.
    Start,
    /// After all existing keys.
    End,
    /// At its alphabetical position.
    Sorted,
}

/// Edit applied to a JSON node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum JsonAction {
    /// Replace the node with a value.
    Set(serde_json::Value),
    /// Remove the node.
    Remove,
    /// Insert a key into the object node.
    #[non_exhaustive]
    Insert {
        /// New key.
        key: String,
        /// New value.
        value: serde_json::Value,
        /// Placement among existing keys.
        position: InsertPosition,
    },
    /// Sort the keys of the object node.
    SortKeys,
    /// Rename the node's key in its parent object.
    RenameKey(String),
}

/// Edit applied to a YAML node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum YamlAction {
    /// Replace the node with a value.
    Set(serde_json::Value),
    /// Remove the node.
    Remove,
    /// Append a value to the sequence node.
    Append(serde_json::Value),
}

/// A single mutation of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum FixOp {
    /// Structured edit of a JSON file.
    #[non_exhaustive]
    Json {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
        /// Path to the target node; empty addresses the root.
        pointer: Vec<PathSeg>,
        /// What to do with the node.
        action: JsonAction,
    },
    /// Structured edit of a YAML file.
    #[non_exhaustive]
    Yaml {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
        /// Path to the target node; empty addresses the root.
        route: Vec<PathSeg>,
        /// What to do with the node.
        action: YamlAction,
    },
    /// Byte-range edit of a text file.
    #[non_exhaustive]
    Text {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
        /// The edit.
        edit: TextEdit,
    },
    /// Create a file.
    #[non_exhaustive]
    CreateFile {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
        /// Full contents.
        contents: String,
    },
    /// Delete a file.
    #[non_exhaustive]
    DeleteFile {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
    },
}

impl FixOp {
    /// A structured edit of the JSON file `path`; `pointer` addresses the node, empty for the root.
    #[must_use]
    pub fn json(path: impl Into<PathBuf>, pointer: Vec<PathSeg>, action: JsonAction) -> Self {
        Self::Json {
            path: path.into(),
            pointer,
            action,
        }
    }

    /// A structured edit of the YAML file `path`; `route` addresses the node, empty for the root.
    #[must_use]
    pub fn yaml(path: impl Into<PathBuf>, route: Vec<PathSeg>, action: YamlAction) -> Self {
        Self::Yaml {
            path: path.into(),
            route,
            action,
        }
    }

    /// A byte-range edit of the text file `path`.
    #[must_use]
    pub fn text(path: impl Into<PathBuf>, edit: TextEdit) -> Self {
        Self::Text {
            path: path.into(),
            edit,
        }
    }

    /// Creates the file `path` with `contents`.
    #[must_use]
    pub fn create_file(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        Self::CreateFile {
            path: path.into(),
            contents: contents.into(),
        }
    }

    /// Deletes the file `path`.
    #[must_use]
    pub fn delete_file(path: impl Into<PathBuf>) -> Self {
        Self::DeleteFile { path: path.into() }
    }
}

/// Replacement of the half-open byte range `[start, end)`; `start == end` inserts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct TextEdit {
    /// First byte replaced.
    pub start: u32,
    /// One past the last byte replaced.
    pub end: u32,
    /// Text put in place of the range.
    pub replacement: String,
}

impl TextEdit {
    /// Replaces the bytes `start..end` with `replacement`.
    #[must_use]
    pub fn new(start: u32, end: u32, replacement: impl Into<String>) -> Self {
        Self {
            start,
            end,
            replacement: replacement.into(),
        }
    }
}

/// The net effect of fixes on one file, ready to be written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", rename_all_fields = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum FileChange {
    /// An existing file is modified.
    Modify {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
        /// Text edits that produced `new_contents`, when the change is textual.
        edits: Vec<TextEdit>,
        /// Full resulting contents.
        new_contents: String,
    },
    /// A file is created.
    Create {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
        /// Full contents.
        contents: String,
    },
    /// A file is deleted.
    Delete {
        /// Workspace-relative file.
        #[serde(serialize_with = "crate::display::serialize_path")]
        path: PathBuf,
    },
}

impl FileChange {
    /// The workspace-relative file the change touches.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Modify { path, .. } | Self::Create { path, .. } | Self::Delete { path } => path,
        }
    }
}

/// Reason a set of [`TextEdit`]s cannot be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EditError {
    /// An edit reaches past the end of the source, or ends before it starts.
    #[error("edit {start}..{end} is out of bounds for source of {len} bytes")]
    OutOfBounds {
        /// Edit start.
        start: u32,
        /// Edit end.
        end: u32,
        /// Source length in bytes.
        len: usize,
    },
    /// An edit boundary falls inside a multibyte character.
    #[error("offset {offset} is not on a character boundary")]
    NotCharBoundary {
        /// The offending offset.
        offset: u32,
    },
    /// Two edits touch overlapping ranges.
    #[error("edit {}..{} overlaps edit {}..{}", first.0, first.1, second.0, second.1)]
    Overlap {
        /// Earlier edit as `(start, end)`.
        first: (u32, u32),
        /// Later edit as `(start, end)`.
        second: (u32, u32),
    },
}

/// Applies `edits` to `source`, independent of the order they are given in.
///
/// Edits must not overlap; adjacent edits are fine. Zero-width inserts at the same offset keep their input order.
///
/// # Errors
///
/// Returns [`EditError`] when an edit is out of bounds, splits a character, or overlaps another edit.
pub fn apply_text_edits(source: &str, edits: &[TextEdit]) -> Result<String, EditError> {
    let ordered = validated(source, edits)?;
    let mut out = String::with_capacity(source.len());
    let cursor = ordered.iter().fold(0, |cursor, edit| {
        out.push_str(&source[cursor..edit.start as usize]);
        out.push_str(&edit.replacement);
        edit.end as usize
    });
    out.push_str(&source[cursor..]);
    Ok(out)
}

fn validated<'a>(source: &str, edits: &'a [TextEdit]) -> Result<Vec<&'a TextEdit>, EditError> {
    edits.iter().try_for_each(|edit| check_bounds(source, edit))?;
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.start, edit.end));
    ordered
        .windows(2)
        .try_for_each(|pair| check_disjoint(pair[0], pair[1]))?;
    Ok(ordered)
}

fn check_bounds(source: &str, edit: &TextEdit) -> Result<(), EditError> {
    if edit.start > edit.end || edit.end as usize > source.len() {
        return Err(EditError::OutOfBounds {
            start: edit.start,
            end: edit.end,
            len: source.len(),
        });
    }
    [edit.start, edit.end]
        .into_iter()
        .find(|&offset| !source.is_char_boundary(offset as usize))
        .map_or(Ok(()), |offset| Err(EditError::NotCharBoundary { offset }))
}

fn check_disjoint(first: &TextEdit, second: &TextEdit) -> Result<(), EditError> {
    if second.start < first.end {
        Err(EditError::Overlap {
            first: (first.start, first.end),
            second: (second.start, second.end),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn constructors_build_the_variants() {
        let pointer = vec![PathSeg::Key("a".into())];
        assert_eq!(
            FixOp::json("p.json", pointer.clone(), JsonAction::Remove),
            FixOp::Json {
                path: "p.json".into(),
                pointer: pointer.clone(),
                action: JsonAction::Remove
            }
        );
        assert_eq!(
            FixOp::yaml("p.yaml", pointer.clone(), YamlAction::Remove),
            FixOp::Yaml {
                path: "p.yaml".into(),
                route: pointer,
                action: YamlAction::Remove
            }
        );
        assert_eq!(
            FixOp::text("t.txt", TextEdit::new(0, 1, "x")),
            FixOp::Text {
                path: "t.txt".into(),
                edit: TextEdit::new(0, 1, "x")
            }
        );
        assert_eq!(
            FixOp::create_file("n.txt", "body"),
            FixOp::CreateFile {
                path: "n.txt".into(),
                contents: "body".into()
            }
        );
        assert_eq!(FixOp::delete_file("d.txt"), FixOp::DeleteFile { path: "d.txt".into() });
        assert_eq!(
            JsonAction::insert("k".into(), serde_json::json!(1), InsertPosition::End),
            JsonAction::Insert {
                key: "k".into(),
                value: serde_json::json!(1),
                position: InsertPosition::End
            }
        );
    }

    fn edit(start: u32, end: u32, replacement: &str) -> TextEdit {
        TextEdit {
            start,
            end,
            replacement: replacement.to_owned(),
        }
    }

    #[test]
    fn applies_unordered_edits() {
        let edits = [edit(6, 11, "there"), edit(0, 5, "howdy")];
        assert_eq!(apply_text_edits("hello world", &edits).unwrap(), "howdy there");
    }

    #[test]
    fn applies_adjacent_edits() {
        let edits = [edit(2, 4, "CD"), edit(0, 2, "AB"), edit(4, 6, "EF")];
        assert_eq!(apply_text_edits("abcdef", &edits).unwrap(), "ABCDEF");
    }

    #[test]
    fn no_edits_returns_source() {
        assert_eq!(apply_text_edits("same", &[]).unwrap(), "same");
    }

    #[test]
    fn inserts_and_deletes() {
        let edits = [edit(0, 0, ">"), edit(1, 3, ""), edit(4, 4, "<")];
        assert_eq!(apply_text_edits("abcd", &edits).unwrap(), ">ad<");
    }

    #[test]
    fn same_offset_inserts_keep_input_order() {
        let edits = [edit(1, 1, "X"), edit(1, 1, "Y"), edit(1, 1, "Z")];
        assert_eq!(apply_text_edits("ab", &edits).unwrap(), "aXYZb");
    }

    #[test]
    fn insert_at_start_of_replacement_comes_first() {
        let edits = [edit(1, 2, "R"), edit(1, 1, "I")];
        assert_eq!(apply_text_edits("abc", &edits).unwrap(), "aIRc");
    }

    #[test]
    fn rejects_invalid_edits() {
        let cases = [
            (
                "abcdef",
                vec![edit(0, 4, "x"), edit(2, 5, "y")],
                EditError::Overlap {
                    first: (0, 4),
                    second: (2, 5),
                },
            ),
            (
                "abcdef",
                vec![edit(1, 3, "x"), edit(2, 2, "y")],
                EditError::Overlap {
                    first: (1, 3),
                    second: (2, 2),
                },
            ),
            (
                "abcdef",
                vec![edit(2, 4, "x"), edit(2, 4, "y")],
                EditError::Overlap {
                    first: (2, 4),
                    second: (2, 4),
                },
            ),
            (
                "abc",
                vec![edit(2, 9, "x")],
                EditError::OutOfBounds {
                    start: 2,
                    end: 9,
                    len: 3,
                },
            ),
            (
                "abc",
                vec![edit(3, 2, "x")],
                EditError::OutOfBounds {
                    start: 3,
                    end: 2,
                    len: 3,
                },
            ),
            (
                "a\u{e9}",
                vec![edit(2, 3, "x")],
                EditError::NotCharBoundary { offset: 2 },
            ),
            (
                "\u{1F600}",
                vec![edit(1, 4, "x")],
                EditError::NotCharBoundary { offset: 1 },
            ),
        ];
        for (source, edits, expected) in cases {
            assert_eq!(apply_text_edits(source, &edits), Err(expected), "source {source:?}");
        }
    }

    #[test]
    fn handles_multibyte_boundaries() {
        let edits = [edit(1, 3, "e")];
        assert_eq!(apply_text_edits("a\u{e9}b", &edits).unwrap(), "aeb");
    }

    fn reference(source: &str, edits: &[TextEdit]) -> String {
        let mut sorted = edits.to_vec();
        sorted.sort_by_key(|e| (e.start, e.end));
        let mut out = source.to_owned();
        for e in sorted.iter().rev() {
            out.replace_range(e.start as usize..e.end as usize, &e.replacement);
        }
        out
    }

    fn build_edits(source: &str, cuts: &[usize], replacements: &[String], rotation: usize) -> Vec<TextEdit> {
        let boundaries: Vec<usize> = source.char_indices().map(|(i, _)| i).chain([source.len()]).collect();
        let mut offsets: Vec<u32> = cuts
            .iter()
            .map(|c| u32::try_from(boundaries[c % boundaries.len()]).unwrap())
            .collect();
        offsets.sort_unstable();
        let mut edits: Vec<TextEdit> = offsets
            .as_chunks::<2>()
            .0
            .iter()
            .zip(replacements)
            .map(|(&[start, end], rep)| edit(start, end, rep))
            .collect();
        if !edits.is_empty() {
            let len = edits.len();
            edits.rotate_left(rotation % len);
        }
        edits
    }

    proptest! {
        #[test]
        fn matches_naive_reference(
            source in "[a-z\u{e9}\u{1F600}\n]{0,24}",
            cuts in proptest::collection::vec(0usize..1000, 0..10),
            replacements in proptest::collection::vec("[xyz\u{20ac}]{0,3}", 5),
            rotation in 0usize..8,
        ) {
            let edits = build_edits(&source, &cuts, &replacements, rotation);
            prop_assert_eq!(apply_text_edits(&source, &edits).unwrap(), reference(&source, &edits));
        }
    }

    #[test]
    fn constructors_fill_every_field() {
        let edit = TextEdit::new(1, 3, "x");
        assert_eq!((edit.start, edit.end, edit.replacement.as_str()), (1, 3, "x"));
        let fix = Fix::new("Title", FixSafety::Unsafe, vec![]);
        assert_eq!(
            fix,
            Fix {
                title: "Title".to_owned(),
                safety: FixSafety::Unsafe,
                ops: Vec::new()
            }
        );
    }
}
