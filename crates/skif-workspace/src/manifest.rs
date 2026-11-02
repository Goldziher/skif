//! `package.json` parsing: one pass produces the JSON value, the line index, the span table and every dependency
//! entry with its parsed specifier.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jsonc_parser::ast::{self, ObjectPropName};
use jsonc_parser::common::{Range, Ranged};
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};
use serde_json::Value;
use skif_core::{DependencyKind, LineIndex, PathSeg, Span};
use skif_specifier::Specifier;

use crate::error::WorkspaceError;

const BOM: char = '\u{feff}';

/// Pointers up to this many segments long are recorded in the span table.
const SPAN_DEPTH: usize = 3;

/// One `"name": "specifier"` entry of a dependency section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEntry {
    /// Which section the entry is in.
    pub kind: DependencyKind,
    /// The dependency name.
    pub name: String,
    /// The specifier exactly as written.
    pub specifier: String,
    /// The specifier parsed once during discovery.
    pub parsed: Arc<Specifier>,
    /// Span of the key string, quotes included.
    pub key_span: Span,
    /// Span of the value string, quotes included.
    pub value_span: Span,
}

/// The spans of one node of a manifest: the whole entry, its key and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpans {
    /// For an object member the key through the value; for an array element or the document, the node itself.
    pub entry: Span,
    /// The key string, quotes included; `None` for array elements and the document.
    pub key: Option<Span>,
    /// The value.
    pub value: Span,
}

/// A parsed `package.json`.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Workspace-relative path with `/` separators, e.g. `packages/a/package.json`.
    pub path: PathBuf,
    /// The file contents exactly as read (a BOM and CRLF line endings are preserved, so spans index this text).
    pub source: Arc<str>,
    /// Offset-to-line/column conversion for `source`.
    pub line_index: Arc<LineIndex>,
    /// The parsed document; key order is preserved.
    pub value: Value,
    spans: HashMap<Box<[PathSeg]>, FieldSpans>,
}

pub(crate) fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn bom_len(source: &str) -> usize {
    if source.starts_with(BOM) { BOM.len_utf8() } else { 0 }
}

fn prop_name<'a, 'b: 'a>(name: &'a ObjectPropName<'b>) -> &'a str {
    match name {
        ObjectPropName::String(lit) => &lit.value,
        ObjectPropName::Word(lit) => lit.value,
    }
}

fn to_json(value: &ast::Value<'_>) -> Value {
    match value {
        ast::Value::StringLit(lit) => Value::String(lit.value.to_string()),
        ast::Value::NumberLit(lit) => serde_json::from_str(lit.value).unwrap_or(Value::Null),
        ast::Value::BooleanLit(lit) => Value::Bool(lit.value),
        ast::Value::NullKeyword(_) => Value::Null,
        ast::Value::Array(array) => Value::Array(array.elements.iter().map(to_json).collect()),
        ast::Value::Object(object) => Value::Object(
            object
                .properties
                .iter()
                .map(|prop| (prop_name(&prop.name).to_owned(), to_json(&prop.value)))
                .collect(),
        ),
    }
}

fn parse_ast(source: &str) -> Result<(ast::Value<'_>, usize), (usize, String)> {
    let bom = bom_len(source);
    let result = parse_to_ast(&source[bom..], &CollectOptions::default(), &ParseOptions::default())
        .map_err(|error| (error.range().start + bom, error.kind().to_string()))?;
    result
        .value
        .map(|value| (value, bom))
        .ok_or_else(|| (bom, "the file is empty".to_owned()))
}

fn invalid(path: &Path, line_index: &LineIndex, offset: usize, message: String) -> WorkspaceError {
    let (line, column) = line_index.line_col(to_u32(offset));
    WorkspaceError::InvalidManifest {
        path: path.to_path_buf(),
        message,
        line,
        column,
    }
}

fn shift(range: Range, bom: usize, line_index: &LineIndex) -> Span {
    line_index.span(to_u32(range.start + bom), to_u32(range.end + bom))
}

struct Spans<'a> {
    bom: usize,
    line_index: &'a LineIndex,
    path: Vec<PathSeg>,
    table: HashMap<Box<[PathSeg]>, FieldSpans>,
}

impl Spans<'_> {
    fn insert(&mut self, entry: Range, key: Option<Range>, value: Range) {
        let spans = FieldSpans {
            entry: shift(entry, self.bom, self.line_index),
            key: key.map(|range| shift(range, self.bom, self.line_index)),
            value: shift(value, self.bom, self.line_index),
        };
        self.table.insert(self.path.as_slice().into(), spans);
    }

    fn record(&mut self, value: &ast::Value<'_>) {
        if self.path.len() >= SPAN_DEPTH {
            return;
        }
        match value {
            ast::Value::Object(object) => {
                for prop in &object.properties {
                    self.path.push(PathSeg::Key(prop_name(&prop.name).to_owned()));
                    let (name, value) = (prop.name.range(), prop.value.range());
                    self.insert(Range::new(name.start, value.end), Some(name), value);
                    self.record(&prop.value);
                    self.path.pop();
                }
            }
            ast::Value::Array(array) => {
                for (index, element) in array.elements.iter().enumerate() {
                    self.path.push(PathSeg::Index(index));
                    self.insert(element.range(), None, element.range());
                    self.record(element);
                    self.path.pop();
                }
            }
            _ => {}
        }
    }
}

/// Drops the entries from `from` on whose name is declared again later in the same section: with duplicate keys the
/// last one wins, as it does for the JSON value and for the span table.
fn keep_last_duplicates(entries: &mut Vec<DependencyEntry>, from: usize) {
    let last: HashMap<&str, usize> = entries[from..]
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.name.as_str(), index))
        .collect();
    let keep: Vec<bool> = entries[from..]
        .iter()
        .enumerate()
        .map(|(index, entry)| last.get(entry.name.as_str()) == Some(&index))
        .collect();
    if keep.iter().all(|&kept| kept) {
        return;
    }
    let mut position = 0;
    entries.retain(|_| {
        let index = position;
        position += 1;
        index < from || keep[index - from]
    });
}

impl Manifest {
    /// Parses a manifest and collects its dependency entries in file order. A name declared twice in one section is
    /// collected once, with the value of its last declaration.
    ///
    /// `path` is the workspace-relative path used in errors and diagnostics. Comments and trailing commas are
    /// tolerated; the root must be an object.
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidManifest`] with the 1-based line and column of the problem.
    pub fn parse(path: PathBuf, source: Arc<str>) -> Result<(Self, Vec<DependencyEntry>), WorkspaceError> {
        let line_index = Arc::new(LineIndex::new(&source));
        let (root, bom) =
            parse_ast(&source).map_err(|(offset, message)| invalid(&path, &line_index, offset, message))?;
        let ast::Value::Object(object) = &root else {
            return Err(invalid(
                &path,
                &line_index,
                root.range().start + bom,
                "expected the manifest to be a JSON object".to_owned(),
            ));
        };

        let mut entries = Vec::new();
        for section in &object.properties {
            let Some(kind) = DependencyKind::from_field(prop_name(&section.name)) else {
                continue;
            };
            let ast::Value::Object(dependencies) = &section.value else {
                continue;
            };
            let section_start = entries.len();
            for dependency in &dependencies.properties {
                if let ast::Value::StringLit(specifier) = &dependency.value {
                    entries.push(DependencyEntry {
                        kind,
                        name: prop_name(&dependency.name).to_owned(),
                        specifier: specifier.value.to_string(),
                        parsed: Specifier::new(&specifier.value),
                        key_span: shift(dependency.name.range(), bom, &line_index),
                        value_span: shift(specifier.range, bom, &line_index),
                    });
                }
            }
            keep_last_duplicates(&mut entries, section_start);
        }

        let mut spans = Spans {
            bom,
            line_index: &line_index,
            path: Vec::new(),
            table: HashMap::new(),
        };
        spans.insert(root.range(), None, root.range());
        spans.record(&root);
        let spans = spans.table;

        let value = to_json(&root);
        Ok((
            Self {
                path,
                source,
                line_index,
                value,
                spans,
            },
            entries,
        ))
    }

    /// The span of the node at `pointer`: for an object key, the whole entry from the key through its value; for an
    /// array index, the element; for an empty pointer, the whole document. `None` when the path does not exist or is
    /// longer than three segments.
    ///
    /// A lookup in the table recorded while parsing; the file is not parsed again. With duplicate keys the last one
    /// wins, as in the JSON value.
    #[must_use]
    pub fn field_span(&self, pointer: &[PathSeg]) -> Option<Span> {
        self.field_spans(pointer).map(|spans| spans.entry)
    }

    /// The span of the key of the object member at `pointer`, quotes included. `None` when the path does not exist,
    /// is longer than three segments, or does not end in an object key (array elements and the document have no key).
    #[must_use]
    pub fn field_key_span(&self, pointer: &[PathSeg]) -> Option<Span> {
        self.field_spans(pointer)?.key
    }

    /// The span of the value at `pointer`, without its key.
    #[must_use]
    pub fn field_value_span(&self, pointer: &[PathSeg]) -> Option<Span> {
        self.field_spans(pointer).map(|spans| spans.value)
    }

    /// The entry, key and value spans of the node at `pointer`, under the same lookup rules as
    /// [`field_span`](Self::field_span).
    #[must_use]
    pub fn field_spans(&self, pointer: &[PathSeg]) -> Option<FieldSpans> {
        self.spans.get(pointer).copied()
    }
}
