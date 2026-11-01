//! `package.json` parsing: one pass produces the JSON value, the line index and every dependency span.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jsonc_parser::ast::{self, ObjectPropName};
use jsonc_parser::common::{Range, Ranged};
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use skif_core::{LineIndex, PathSeg, Span};

use crate::error::WorkspaceError;

const BOM: char = '\u{feff}';

/// The dependency sections of a `package.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyKind {
    /// `dependencies`
    Dependencies,
    /// `devDependencies`
    DevDependencies,
    /// `peerDependencies`
    PeerDependencies,
    /// `optionalDependencies`
    OptionalDependencies,
}

impl DependencyKind {
    /// Every kind, in the order manifests conventionally list them.
    pub const ALL: [Self; 4] = [
        Self::Dependencies,
        Self::DevDependencies,
        Self::PeerDependencies,
        Self::OptionalDependencies,
    ];

    /// The `package.json` field name, e.g. `"devDependencies"`.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Dependencies => "dependencies",
            Self::DevDependencies => "devDependencies",
            Self::PeerDependencies => "peerDependencies",
            Self::OptionalDependencies => "optionalDependencies",
        }
    }

    fn from_field(field: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.field() == field)
    }
}

/// One `"name": "specifier"` entry of a dependency section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEntry {
    /// Which section the entry is in.
    pub kind: DependencyKind,
    /// The dependency name.
    pub name: String,
    /// The specifier exactly as written.
    pub specifier: String,
    /// Span of the key string, quotes included.
    pub key_span: Span,
    /// Span of the value string, quotes included.
    pub value_span: Span,
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

impl Manifest {
    /// Parses a manifest and collects its dependency entries in file order.
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
            for dependency in &dependencies.properties {
                if let ast::Value::StringLit(specifier) = &dependency.value {
                    entries.push(DependencyEntry {
                        kind,
                        name: prop_name(&dependency.name).to_owned(),
                        specifier: specifier.value.to_string(),
                        key_span: shift(dependency.name.range(), bom, &line_index),
                        value_span: shift(specifier.range, bom, &line_index),
                    });
                }
            }
        }

        let value = to_json(&root);
        Ok((
            Self {
                path,
                source,
                line_index,
                value,
            },
            entries,
        ))
    }

    /// The span of the node at `pointer`: for an object key, the whole entry from the key through its value; for an
    /// array index, the element; for an empty pointer, the whole document. `None` when the path does not exist.
    #[must_use]
    pub fn field_span(&self, pointer: &[PathSeg]) -> Option<Span> {
        let (root, bom) = parse_ast(&self.source).ok()?;
        let mut current = &root;
        let mut range = root.range();
        for segment in pointer {
            match (segment, current) {
                (PathSeg::Key(key), ast::Value::Object(object)) => {
                    let prop = object
                        .properties
                        .iter()
                        .rev()
                        .find(|prop| prop_name(&prop.name) == key.as_str())?;
                    range = Range::new(prop.name.range().start, prop.value.range().end);
                    current = &prop.value;
                }
                (PathSeg::Index(index), ast::Value::Array(array)) => {
                    let element = array.elements.get(*index)?;
                    range = element.range();
                    current = element;
                }
                _ => return None,
            }
        }
        Some(shift(range, bom, &self.line_index))
    }
}
