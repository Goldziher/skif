//! `package.json` parsing: one pass produces the JSON value, the line index, the span table and every dependency
//! entry with its parsed specifier.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use jsonc_parser::ast::{self, ObjectPropName};
use jsonc_parser::common::{Range, Ranged};
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};
use serde_json::Value;
use skif_core::HashMap;
use skif_core::{DependencyKind, LineIndex, PathSeg, Span};
use skif_specifier::{Specifier, SpecifierCache};

use crate::error::WorkspaceError;

mod fast;

const BOM: char = '\u{feff}';

/// Pointers up to this many segments long are recorded in the span table.
const SPAN_DEPTH: usize = 3;

/// One `"name": "specifier"` entry of a dependency section.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DependencyEntry {
    /// Which section the entry is in.
    pub kind: DependencyKind,
    /// The dependency name, shared: a consumer that keeps it clones the `Arc` instead of the text.
    pub name: Arc<str>,
    /// The specifier exactly as written, shared like [`name`](Self::name).
    pub specifier: Arc<str>,
    /// The specifier parsed once during discovery.
    pub parsed: Arc<Specifier>,
    /// Span of the key string, quotes included.
    pub key_span: Span,
    /// Span of the value string, quotes included.
    pub value_span: Span,
}

/// The spans of one node of a manifest: the whole entry, its key and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldSpans {
    /// For an object member the key through the value; for an array element or the document, the node itself.
    pub entry: Span,
    /// The key string, quotes included; `None` for array elements and the document.
    pub key: Option<Span>,
    /// The value.
    pub value: Span,
}

/// A parsed `package.json`.
///
/// The text, its line index, the parsed value and the span table belong together: spans index the text, and the
/// value is parsed from it. They are read through accessors so a host cannot change one without the others.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Manifest {
    /// Workspace-relative path with `/` separators, e.g. `packages/a/package.json`.
    pub path: PathBuf,
    pub(crate) source: Arc<str>,
    pub(crate) line_index: Arc<LineIndex>,
    pub(crate) value: Arc<Value>,
    /// Built on the first span lookup: most manifests of a clean workspace never need one.
    spans: OnceLock<HashMap<Box<[PathSeg]>, FieldSpans>>,
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

/// The JSON value of a manifest read the way [`Manifest::parse`] reads it: a BOM, comments and trailing commas are
/// tolerated. `None` when the text is not a JSON object.
pub(crate) fn parse_value(source: &str) -> Option<Value> {
    let (root, _) = parse_ast(source).ok()?;
    matches!(root, ast::Value::Object(_)).then(|| to_json(&root))
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

/// The span table of a manifest that parsed once already.
fn span_table(source: &str, line_index: &LineIndex) -> HashMap<Box<[PathSeg]>, FieldSpans> {
    let Ok((root, bom)) = parse_ast(source) else {
        return HashMap::default();
    };
    let mut spans = Spans {
        bom,
        line_index,
        path: Vec::new(),
        table: HashMap::default(),
    };
    spans.insert(root.range(), None, root.range());
    spans.record(&root);
    spans.table
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
        .map(|(index, entry)| (&*entry.name, index))
        .collect();
    let keep: Vec<bool> = entries[from..]
        .iter()
        .enumerate()
        .map(|(index, entry)| last.get(&*entry.name) == Some(&index))
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

/// Collects the dependency entries of a manifest while its top-level members are read, in file order.
struct Collector<'c> {
    bom: usize,
    line_index: &'c LineIndex,
    cache: &'c SpecifierCache,
    entries: Vec<DependencyEntry>,
    seen: Vec<DependencyKind>,
}

impl<'c> Collector<'c> {
    fn new(bom: usize, line_index: &'c LineIndex, cache: &'c SpecifierCache) -> Self {
        Self {
            bom,
            line_index,
            cache,
            entries: Vec::new(),
            seen: Vec::new(),
        }
    }

    /// A top-level member named as the section `kind`, whatever its value. A section written twice: the last
    /// occurrence replaces the first as a whole, as it does in the value.
    fn section(&mut self, kind: DependencyKind) {
        if self.seen.contains(&kind) {
            self.entries.retain(|entry| entry.kind != kind);
        } else {
            self.seen.push(kind);
        }
    }

    /// A member of the section `kind` whose value is the string `specifier`; `key` and `value` are the ranges of the
    /// two strings, quotes included, in the text after a BOM.
    fn entry(&mut self, kind: DependencyKind, name: &str, key: Range, specifier: &str, value: Range) {
        self.entries.push(DependencyEntry {
            kind,
            name: name.into(),
            specifier: specifier.into(),
            parsed: self.cache.get_or_parse(specifier),
            key_span: shift(key, self.bom, self.line_index),
            value_span: shift(value, self.bom, self.line_index),
        });
    }

    /// The entries from `start` on are those of one section object: with duplicate keys the last one wins.
    fn end_section(&mut self, start: usize) {
        keep_last_duplicates(&mut self.entries, start);
    }
}

/// The value of a manifest read with `jsonc-parser`, which tolerates comments, trailing commas and the other
/// leniencies of JSONC and names the position of an error, and its dependency entries handed to `collector`.
fn read(path: &Path, source: &str, collector: &mut Collector<'_>) -> Result<Value, WorkspaceError> {
    let line_index = collector.line_index;
    let (root, bom) = parse_ast(source).map_err(|(offset, message)| invalid(path, line_index, offset, message))?;
    let ast::Value::Object(object) = &root else {
        return Err(invalid(
            path,
            line_index,
            root.range().start + bom,
            "expected the manifest to be a JSON object".to_owned(),
        ));
    };
    for section in &object.properties {
        let Some(kind) = DependencyKind::from_field(prop_name(&section.name)) else {
            continue;
        };
        collector.section(kind);
        let ast::Value::Object(dependencies) = &section.value else {
            continue;
        };
        let start = collector.entries.len();
        // Exactly the room the section needs: a workspace keeps every package's entries for the whole run.
        collector.entries.reserve_exact(dependencies.properties.len());
        for dependency in &dependencies.properties {
            if let ast::Value::StringLit(specifier) = &dependency.value {
                collector.entry(
                    kind,
                    prop_name(&dependency.name),
                    dependency.name.range(),
                    &specifier.value,
                    specifier.range,
                );
            }
        }
        collector.end_section(start);
    }
    Ok(to_json(&root))
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
        Self::parse_with(path, source, &SpecifierCache::new())
    }

    /// Like [`parse`](Self::parse), parsing each distinct specifier once through `cache`: `^18.3.1` written in a
    /// hundred manifests is parsed and stored once.
    ///
    /// # Errors
    /// As [`parse`](Self::parse).
    pub fn parse_with(
        path: PathBuf,
        source: Arc<str>,
        cache: &SpecifierCache,
    ) -> Result<(Self, Vec<DependencyEntry>), WorkspaceError> {
        let line_index = Arc::new(LineIndex::new(&source));
        let bom = bom_len(&source);
        let mut collector = Collector::new(bom, &line_index, cache);
        let value = if let Some(value) = fast::parse(&source[bom..], &mut collector) {
            // Exactly the room the entries need: a workspace keeps every package's entries for the whole run.
            collector.entries.shrink_to_fit();
            value
        } else {
            collector = Collector::new(bom, &line_index, cache);
            read(&path, &source, &mut collector)?
        };
        let entries = collector.entries;
        let value = Arc::new(value);
        Ok((
            Self {
                path,
                source,
                line_index,
                value,
                spans: OnceLock::new(),
            },
            entries,
        ))
    }

    /// The file contents exactly as read (a BOM and CRLF line endings are preserved, so spans index this text).
    #[must_use]
    pub fn source(&self) -> &Arc<str> {
        &self.source
    }

    /// Offset-to-line/column conversion for [`source`](Self::source).
    #[must_use]
    pub fn line_index(&self) -> &Arc<LineIndex> {
        &self.line_index
    }

    /// The parsed document; key order is preserved.
    ///
    /// `serde_json::Value` is part of the data model of this crate on purpose: it is the one JSON representation
    /// every consumer already has.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The parsed document, shared: cloning the handle does not copy the document.
    #[must_use]
    pub fn shared_value(&self) -> Arc<Value> {
        Arc::clone(&self.value)
    }

    /// The span of the node at `pointer`: for an object key, the whole entry from the key through its value; for an
    /// array index, the element; for an empty pointer, the whole document. `None` when the path does not exist or is
    /// longer than three segments.
    ///
    /// A lookup in a table built from the file on the first query, so manifests nobody asks about pay nothing for
    /// it. With duplicate keys the last one wins, as in the JSON value.
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
        self.spans
            .get_or_init(|| span_table(&self.source, &self.line_index))
            .get(pointer)
            .copied()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    type Parsed = (Value, Vec<DependencyEntry>);

    /// The value and entries of `source` read by the strict reader, when it takes the text.
    fn strict(source: &str) -> Option<Parsed> {
        let line_index = LineIndex::new(source);
        let cache = SpecifierCache::new();
        let bom = bom_len(source);
        let mut collector = Collector::new(bom, &line_index, &cache);
        let value = fast::parse(&source[bom..], &mut collector)?;
        Some((value, collector.entries))
    }

    /// The value and entries of `source` read by `jsonc-parser`, or the error message.
    fn lenient(source: &str) -> Result<Parsed, String> {
        let line_index = LineIndex::new(source);
        let cache = SpecifierCache::new();
        let mut collector = Collector::new(bom_len(source), &line_index, &cache);
        let value = read(Path::new("package.json"), source, &mut collector).map_err(|error| error.to_string())?;
        Ok((value, collector.entries))
    }

    /// Every `package.json` below `dir`.
    fn manifests(dir: &Path, found: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "node_modules") {
                    manifests(&path, found);
                }
            } else if path.file_name().is_some_and(|name| name == "package.json") {
                found.push(path);
            }
        }
    }

    #[test]
    fn the_strict_reader_agrees_with_jsonc_parser() {
        let crafted = [
            "{}",
            "\u{feff}{\"name\":\"a\"}",
            " \t\r\n{ \"name\" : \"a\" , \"version\":\"1.0.0\" }\n",
            r#"{"dependencies":{"a":"^1.0.0","b":"2","a":"3"}}"#,
            r#"{"dependencies":{"a":"1"},"devDependencies":{"b":"2"},"dependencies":{"c":"3"}}"#,
            r#"{"dependencies":"oops","dependencies":{"c":"3"}}"#,
            r#"{"dependencies":{"c":"3"},"dependencies":[]}"#,
            r#"{"dependencies":{"a":1,"b":null,"c":{"d":"e"},"f":["g"],"h":"npm:x@1"}}"#,
            r#"{"peerDependencies":{"a":"*"},"optionalDependencies":{},"overrides":{"a":"1"}}"#,
            r#"{"n":[0,-0,1.5,-2e10,3E+2,4e-2,1e400,12345678901234567890,true,false,null,[],[[]],{}]}"#,
            "{\"name\":\"é ünïcode ✓\",\"x\":\"\u{7f}\"}",
            r#"{"a":{"b":{"c":{"d":[1,{"e":"f"}]}}},"a":2}"#,
        ];
        let mut sources: Vec<String> = crafted.iter().map(|&text| text.to_owned()).collect();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for dir in ["fixtures", "crates"] {
            manifests(&root.join(dir), &mut files);
        }
        assert!(files.len() > 50, "{} manifests", files.len());
        sources.extend(files.iter().map(|path| std::fs::read_to_string(path).unwrap()));
        let mut taken = 0;
        for source in &sources {
            if let Some(parsed) = strict(source) {
                taken += 1;
                assert_eq!(Ok(parsed), lenient(source), "{source}");
            }
        }
        // The strict reader is not vacuous: it takes the crafted documents and most real manifests.
        assert!(taken > sources.len() / 2, "{taken} of {}", sources.len());
        for text in crafted {
            assert!(strict(text).is_some(), "{text}");
        }
    }

    #[test]
    fn the_strict_reader_leaves_jsonc_and_errors_to_jsonc_parser() {
        let lenient_only = [
            "",
            "   ",
            "[]",
            "\"a\"",
            "{} {}",
            r#"{"a":1,}"#,
            r#"{"a":[1,]}"#,
            "{\"a\":1 // comment\n}",
            "{/* c */\"a\":1}",
            "{a:1}",
            "{'a':1}",
            r#"{"a":"\"quoted\""}"#,
            r#"{"a\n":1}"#,
            "{\"a\":\"tab\there\"}",
            r#"{"a":01}"#,
            r#"{"a":1.}"#,
            r#"{"a":.5}"#,
            r#"{"a":+1}"#,
            r#"{"a":0x1F}"#,
            r#"{"a":truex}"#,
            r#"{"a":tru}"#,
            r#"{"a":1"#,
            r#"{"a""#,
            "{\"a\":\u{a0}1}",
        ];
        for text in lenient_only {
            assert!(strict(text).is_none(), "{text}");
        }
        let deep = format!("{{\"a\":{}1{}}}", "[".repeat(100), "]".repeat(100));
        assert!(strict(&deep).is_none());
        assert!(lenient(&deep).is_ok());
    }
}
