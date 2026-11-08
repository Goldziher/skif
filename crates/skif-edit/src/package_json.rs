//! `package.json` formatting as syncpack's `format` does it, decided by ADR 0050.
//!
//! The manifest is read into a tree that keeps the source text of every scalar and every gap between members. Steps
//! reorder members (or collapse a shorthand field); rendering puts each member's text back into the gaps it came from,
//! so a file keeps its layout. With an indent, the tree is rendered the way `JSON.stringify(value, null, indent)` lays
//! it out instead.

use jsonc_parser::ast::{self, ObjectPropName};
use jsonc_parser::common::Ranged;
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};
use skif_core::TextEdit;

use crate::{EditPlanError, FileKind};

const BOM: char = '\u{feff}';

/// What [`format_package_json`] checks, with syncpack's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FormatOptions {
    /// Top-level keys that come first, in this order.
    pub sort_first: Vec<String>,
    /// Fields sorted one level deep: an object by key, an array of strings by value. `a/b` addresses a nested field.
    pub sort_az: Vec<String>,
    /// Keys that come first, in this order, in every object of `exports`; `default` is always last. Moving a key
    /// other than `types` and `default` changes which condition matches first, so it makes the change unsafe.
    pub sort_exports: Vec<String>,
    /// Sort the top-level keys `sort_first` does not name in byte order.
    pub sort_packages: bool,
    /// Collapse a `bugs` object holding only `url` to that string.
    pub format_bugs: bool,
    /// Collapse a `repository` object holding only `url` and a `type` of `git` (or no `type`) to the URL string.
    pub format_repository: bool,
    /// When set, the file must be laid out as `JSON.stringify(value, null, indent)` lays it out. A file that is not
    /// strict JSON (comments, trailing commas, single quotes) keeps its layout, since re-rendering it would drop the
    /// comments or write invalid JSON.
    pub indent: Option<String>,
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            sort_first: strings(&DEFAULT_SORT_FIRST),
            sort_az: strings(&DEFAULT_SORT_AZ),
            sort_exports: strings(&DEFAULT_SORT_EXPORTS),
            sort_packages: true,
            format_bugs: false,
            format_repository: false,
            indent: None,
        }
    }
}

/// syncpack's default `sortFirst`.
pub const DEFAULT_SORT_FIRST: [&str; 4] = ["name", "description", "version", "author"];

/// syncpack's default `sortAz`.
pub const DEFAULT_SORT_AZ: [&str; 8] = [
    "bin",
    "contributors",
    "dependencies",
    "devDependencies",
    "keywords",
    "peerDependencies",
    "resolutions",
    "scripts",
];

/// The default `sort-exports`: `types` first and `default` last, every other condition where it is.
///
/// Node and bundlers take the first condition of an object that matches, so moving any other key changes how an
/// import resolves. TypeScript reads `types` only as a condition it matches first, and nothing after `default` is
/// ever reached, so these two moves are the ones that cannot break a working package. syncpack's longer default list
/// can be configured; it reorders the other conditions too, and that change is unsafe.
pub const DEFAULT_SORT_EXPORTS: [&str; 2] = ["types", "default"];

impl FormatOptions {
    /// Sets the keys that come first.
    #[must_use]
    pub fn with_sort_first(mut self, keys: Vec<String>) -> Self {
        self.sort_first = keys;
        self
    }

    /// Sets the fields sorted alphabetically.
    #[must_use]
    pub fn with_sort_az(mut self, keys: Vec<String>) -> Self {
        self.sort_az = keys;
        self
    }

    /// Sets the order of `exports` conditions.
    #[must_use]
    pub fn with_sort_exports(mut self, keys: Vec<String>) -> Self {
        self.sort_exports = keys;
        self
    }

    /// Sets whether the remaining top-level keys are sorted.
    #[must_use]
    pub const fn with_sort_packages(mut self, sort: bool) -> Self {
        self.sort_packages = sort;
        self
    }

    /// Sets whether `bugs` is collapsed.
    #[must_use]
    pub const fn with_format_bugs(mut self, format: bool) -> Self {
        self.format_bugs = format;
        self
    }

    /// Sets whether `repository` is collapsed.
    #[must_use]
    pub const fn with_format_repository(mut self, format: bool) -> Self {
        self.format_repository = format;
        self
    }

    /// Sets the indent the layout is checked against; `None` keeps the layout.
    #[must_use]
    pub fn with_indent(mut self, indent: Option<String>) -> Self {
        self.indent = indent;
        self
    }
}

/// One way a manifest differs from its formatted form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum FormatIssue {
    /// The top-level keys are not in the configured order.
    KeyOrder,
    /// `bugs` is an object that has a shorthand.
    Bugs,
    /// `repository` is an object that has a shorthand.
    Repository,
    /// `types` is not the first or `default` not the last condition of an object in `exports`.
    Exports,
    /// The conditions of `exports` are not in the configured order, and putting them in it changes which condition
    /// matches first: the change is unsafe.
    ExportsReordered,
    /// The named field is not sorted.
    SortAz(String),
    /// The layout differs from the configured indent.
    Layout,
}

impl FormatIssue {
    /// A short sentence naming the difference.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::KeyOrder => "the top-level keys are not in the configured order".to_owned(),
            Self::Bugs => "`bugs` can be written as its URL".to_owned(),
            Self::Repository => "`repository` can be written as its shorthand".to_owned(),
            Self::Exports => "the conditions of `exports` are not in the configured order".to_owned(),
            Self::ExportsReordered => {
                "the conditions of `exports` are not in the configured order, and reordering them can change how \
                 imports resolve"
                    .to_owned()
            }
            Self::SortAz(field) => format!("`{field}` is not sorted alphabetically"),
            Self::Layout => "the layout differs from the configured indent".to_owned(),
        }
    }

    /// Whether the change cannot alter how the package behaves; only [`Self::ExportsReordered`] can.
    #[must_use]
    pub const fn is_safe(&self) -> bool {
        !matches!(self, Self::ExportsReordered)
    }
}

/// The result of [`format_package_json`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Formatted {
    /// Every difference found, in the order the steps run; empty when the file is formatted.
    pub issues: Vec<FormatIssue>,
    /// The formatted text; equal to the source when `issues` is empty.
    pub output: String,
}

/// Formats a `package.json` source as ADR 0050 describes.
///
/// # Errors
///
/// Returns [`EditPlanError::Parse`] when the source is not JSON, and [`EditPlanError::WrongNodeKind`] when its root is
/// not an object.
pub fn format_package_json(source: &str, options: &FormatOptions) -> Result<Formatted, EditPlanError> {
    let mut doc = Doc::parse(source)?;
    let Node::Object(root) = &mut doc.root else {
        return Err(EditPlanError::WrongNodeKind {
            op: "format package.json".to_owned(),
            expected: "an object",
        });
    };
    let mut issues = Vec::new();
    if (options.sort_packages || !options.sort_first.is_empty()) && sort_top_level(root, options) {
        issues.push(FormatIssue::KeyOrder);
    }
    if options.format_bugs && collapse_bugs(root) {
        issues.push(FormatIssue::Bugs);
    }
    if options.format_repository && collapse_repository(root) {
        issues.push(FormatIssue::Repository);
    }
    if !options.sort_exports.is_empty()
        && let Some(exports) = root.get_mut("exports")
    {
        let mut change = ExportsChange::default();
        sort_exports(exports, &options.sort_exports, &mut change);
        if change.reordered {
            issues.push(FormatIssue::ExportsReordered);
        } else if change.moved {
            issues.push(FormatIssue::Exports);
        }
    }
    for field in &options.sort_az {
        if let Some(value) = root.find_mut(field)
            && sort_az(value)
        {
            issues.push(FormatIssue::SortAz(field.clone()));
        }
    }
    let kept = doc.render_kept();
    let output = match &options.indent {
        Some(indent) if is_strict_json(source) => {
            let canonical = doc.render_canonical(indent, newline(source));
            if canonical != kept {
                issues.push(FormatIssue::Layout);
            }
            canonical
        }
        _ => kept,
    };
    Ok(Formatted { issues, output })
}

/// The smallest single edit that turns `before` into `after`, or `None` when they are equal.
#[must_use]
pub fn minimal_edit(before: &str, after: &str) -> Option<TextEdit> {
    if before == after {
        return None;
    }
    let (a, b) = (before.as_bytes(), after.as_bytes());
    let mut start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !(before.is_char_boundary(start) && after.is_char_boundary(start)) {
        start -= 1;
    }
    let limit = a.len().min(b.len()) - start;
    let mut tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(limit)
        .take_while(|(x, y)| x == y)
        .count();
    while !(before.is_char_boundary(a.len() - tail) && after.is_char_boundary(b.len() - tail)) {
        tail -= 1;
    }
    let end = a.len() - tail;
    Some(TextEdit::new(
        u32::try_from(start).ok()?,
        u32::try_from(end).ok()?,
        &after[start..b.len() - tail],
    ))
}

/// Whether `source` is plain JSON, the input the canonical layout can render without losing anything.
fn is_strict_json(source: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(source.strip_prefix(BOM).unwrap_or(source)).is_ok()
}

fn newline(source: &str) -> &'static str {
    if source.contains("\r\n") { "\r\n" } else { "\n" }
}

/// A JSON value with the source text it was read from.
#[derive(Debug, Clone)]
enum Node {
    Object(Object),
    Array(Array),
    Scalar {
        /// The literal as written.
        text: String,
        /// The decoded value of a string literal.
        string: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct Object {
    /// The text before each member and after the last one; one more than `members`.
    gaps: Vec<String>,
    members: Vec<Member>,
}

#[derive(Debug, Clone)]
struct Member {
    key: String,
    /// The key literal as written, quotes included.
    key_text: String,
    /// The text from the start of the key to the start of the value: key, colon and spaces.
    head: String,
    value: Node,
}

#[derive(Debug, Clone)]
struct Array {
    /// The text before each item and after the last one; one more than `items`.
    gaps: Vec<String>,
    items: Vec<Node>,
}

impl Object {
    fn get_mut(&mut self, key: &str) -> Option<&mut Node> {
        self.members
            .iter_mut()
            .find(|member| member.key == key)
            .map(|member| &mut member.value)
    }

    fn get(&self, key: &str) -> Option<&Node> {
        self.members
            .iter()
            .find(|member| member.key == key)
            .map(|member| &member.value)
    }

    /// The node at `path`, a `/`-separated list of keys.
    fn find_mut(&mut self, path: &str) -> Option<&mut Node> {
        let mut keys = path.split('/');
        let mut node = self.get_mut(keys.next()?)?;
        for key in keys {
            let Node::Object(object) = node else {
                return None;
            };
            node = object.get_mut(key)?;
        }
        Some(node)
    }

    /// Reorders the members to `order`, a permutation of their indices; `true` when the order changed.
    fn reorder(&mut self, order: &[usize]) -> bool {
        if order.iter().enumerate().all(|(position, index)| position == *index) {
            return false;
        }
        let mut taken: Vec<Option<Member>> = self.members.drain(..).map(Some).collect();
        self.members = order.iter().filter_map(|index| taken[*index].take()).collect();
        true
    }
}

struct Doc {
    /// The byte order mark and whitespace before the root value.
    prefix: String,
    root: Node,
    suffix: String,
}

fn parse_error(message: impl Into<String>) -> EditPlanError {
    EditPlanError::Parse {
        kind: FileKind::Json,
        message: message.into(),
    }
}

impl Doc {
    fn parse(source: &str) -> Result<Self, EditPlanError> {
        let bom = if source.starts_with(BOM) { BOM.len_utf8() } else { 0 };
        let body = &source[bom..];
        let parsed = parse_to_ast(body, &CollectOptions::default(), &ParseOptions::default())
            .map_err(|error| parse_error(error.to_string()))?;
        let root = parsed.value.ok_or_else(|| parse_error("the file is empty"))?;
        let range = root.range();
        Ok(Self {
            prefix: source[..bom + range.start].to_owned(),
            root: node(body, &root),
            suffix: body[range.end..].to_owned(),
        })
    }

    fn render_kept(&self) -> String {
        let mut out = self.prefix.clone();
        kept(&self.root, &mut out);
        out.push_str(&self.suffix);
        out
    }

    fn render_canonical(&self, indent: &str, newline: &str) -> String {
        let mut out = String::new();
        if self.prefix.starts_with(BOM) {
            out.push(BOM);
        }
        canonical(&self.root, indent, newline, 0, &mut out);
        out.push_str(newline);
        out
    }
}

fn prop_name(name: &ObjectPropName<'_>) -> String {
    match name {
        ObjectPropName::String(lit) => lit.value.to_string(),
        ObjectPropName::Word(lit) => lit.value.to_owned(),
    }
}

/// Splits `text[start..end]` around `spans` into the gaps between them.
fn gaps(text: &str, start: usize, end: usize, spans: &[(usize, usize)]) -> Vec<String> {
    let mut out = Vec::with_capacity(spans.len() + 1);
    let mut at = start;
    for (span_start, span_end) in spans {
        out.push(text[at..*span_start].to_owned());
        at = *span_end;
    }
    out.push(text[at..end].to_owned());
    out
}

fn node(text: &str, value: &ast::Value<'_>) -> Node {
    match value {
        ast::Value::Object(object) => {
            let spans: Vec<(usize, usize)> = object
                .properties
                .iter()
                .map(|prop| (prop.range.start, prop.range.end))
                .collect();
            let members = object
                .properties
                .iter()
                .map(|prop| {
                    let key_range = prop.name.range();
                    Member {
                        key: prop_name(&prop.name),
                        key_text: text[key_range.start..key_range.end].to_owned(),
                        head: text[prop.range.start..prop.value.range().start].to_owned(),
                        value: node(text, &prop.value),
                    }
                })
                .collect();
            Node::Object(Object {
                gaps: gaps(text, object.range.start + 1, object.range.end - 1, &spans),
                members,
            })
        }
        ast::Value::Array(array) => {
            let spans: Vec<(usize, usize)> = array
                .elements
                .iter()
                .map(|item| (item.range().start, item.range().end))
                .collect();
            Node::Array(Array {
                gaps: gaps(text, array.range.start + 1, array.range.end - 1, &spans),
                items: array.elements.iter().map(|item| node(text, item)).collect(),
            })
        }
        other => {
            let range = other.range();
            Node::Scalar {
                text: text[range.start..range.end].to_owned(),
                string: match other {
                    ast::Value::StringLit(lit) => Some(lit.value.to_string()),
                    _ => None,
                },
            }
        }
    }
}

/// Renders `node` back into the gaps it was read with.
fn kept(node: &Node, out: &mut String) {
    match node {
        Node::Object(object) => {
            out.push('{');
            for (gap, member) in object.gaps.iter().zip(&object.members) {
                out.push_str(gap);
                out.push_str(&member.head);
                kept(&member.value, out);
            }
            out.push_str(object.gaps.last().map_or("", String::as_str));
            out.push('}');
        }
        Node::Array(array) => {
            out.push('[');
            for (gap, item) in array.gaps.iter().zip(&array.items) {
                out.push_str(gap);
                kept(item, out);
            }
            out.push_str(array.gaps.last().map_or("", String::as_str));
            out.push(']');
        }
        Node::Scalar { text, .. } => out.push_str(text),
    }
}

/// Renders `node` as `JSON.stringify(value, null, indent)` would, keeping the text of scalars.
fn canonical(node: &Node, indent: &str, newline: &str, depth: usize, out: &mut String) {
    let open = |out: &mut String, index: usize| {
        if index > 0 {
            out.push(',');
        }
        out.push_str(newline);
        out.push_str(&indent.repeat(depth + 1));
    };
    let close = |out: &mut String| {
        out.push_str(newline);
        out.push_str(&indent.repeat(depth));
    };
    match node {
        Node::Object(object) if object.members.is_empty() => out.push_str("{}"),
        Node::Array(array) if array.items.is_empty() => out.push_str("[]"),
        Node::Object(object) => {
            out.push('{');
            for (index, member) in object.members.iter().enumerate() {
                open(out, index);
                if member.key_text.starts_with('"') {
                    out.push_str(&member.key_text);
                } else {
                    out.push_str(&quote(&member.key));
                }
                out.push_str(": ");
                canonical(&member.value, indent, newline, depth + 1, out);
            }
            close(out);
            out.push('}');
        }
        Node::Array(array) => {
            out.push('[');
            for (index, item) in array.items.iter().enumerate() {
                open(out, index);
                canonical(item, indent, newline, depth + 1, out);
            }
            close(out);
            out.push(']');
        }
        Node::Scalar { text, .. } => out.push_str(text),
    }
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
}

/// `sort_first` keys first, then the rest in byte order when `sort_packages` is on.
fn sort_top_level(root: &mut Object, options: &FormatOptions) -> bool {
    let mut order: Vec<usize> = Vec::with_capacity(root.members.len());
    for key in &options.sort_first {
        for (index, member) in root.members.iter().enumerate() {
            if &member.key == key && !order.contains(&index) {
                order.push(index);
            }
        }
    }
    let mut rest: Vec<usize> = (0..root.members.len()).filter(|index| !order.contains(index)).collect();
    if options.sort_packages {
        rest.sort_by(|a, b| root.members[*a].key.cmp(&root.members[*b].key));
    }
    order.extend(rest);
    root.reorder(&order)
}

/// The string value of `node`, when it is a string literal.
fn string_value(node: &Node) -> Option<&str> {
    match node {
        Node::Scalar { string, .. } => string.as_deref(),
        _ => None,
    }
}

/// `{ "url": "…" }` becomes `"…"`.
fn collapse_bugs(root: &mut Object) -> bool {
    let Some(node @ Node::Object(_)) = root.get_mut("bugs") else {
        return false;
    };
    let Node::Object(bugs) = &*node else {
        return false;
    };
    let [member] = bugs.members.as_slice() else {
        return false;
    };
    if member.key != "url" || string_value(&member.value).is_none() {
        return false;
    }
    *node = member.value.clone();
    true
}

/// `{ "type": "git", "url": "git+https://github.com/o/r.git" }` becomes `"o/r.git"`.
fn collapse_repository(root: &mut Object) -> bool {
    let Some(node @ Node::Object(_)) = root.get_mut("repository") else {
        return false;
    };
    let Node::Object(repository) = &*node else {
        return false;
    };
    if repository
        .members
        .iter()
        .any(|member| member.key != "url" && member.key != "type")
    {
        return false;
    }
    // The shorthand means a git repository: an `svn` or `hg` one would lose its type.
    if let Some(kind) = repository.get("type")
        && string_value(kind) != Some("git")
    {
        return false;
    }
    let Some(url_node) = repository.get("url") else {
        return false;
    };
    let Some(url) = string_value(url_node) else {
        return false;
    };
    // syncpack removes the match of `.+github\.com/`: everything up to the last `github.com/` after the first byte.
    let replacement = match url.rfind("github.com/") {
        Some(at) if at > 0 => {
            let short = &url[at + "github.com/".len()..];
            Node::Scalar {
                text: quote(short),
                string: Some(short.to_owned()),
            }
        }
        _ => url_node.clone(),
    };
    *node = replacement;
    true
}

/// What [`sort_exports`] did.
#[derive(Debug, Default)]
struct ExportsChange {
    /// Some object of `exports` changed order.
    moved: bool,
    /// Some object changed the relative order of conditions other than `types` and `default`.
    reordered: bool,
}

/// In every object of `exports`: the listed keys first, in order, the rest as they were, and `default` last.
fn sort_exports(node: &mut Node, order: &[String], change: &mut ExportsChange) {
    let Node::Object(object) = node else {
        return;
    };
    let mut positions: Vec<usize> = Vec::with_capacity(object.members.len());
    for key in order {
        for (index, member) in object.members.iter().enumerate() {
            if &member.key == key && member.key != "default" && !positions.contains(&index) {
                positions.push(index);
            }
        }
    }
    let defaults: Vec<usize> = (0..object.members.len())
        .filter(|index| object.members[*index].key == "default")
        .collect();
    let rest: Vec<usize> = (0..object.members.len())
        .filter(|index| !positions.contains(index) && !defaults.contains(index))
        .collect();
    positions.extend(rest);
    positions.extend(defaults);
    let others: Vec<usize> = positions
        .iter()
        .copied()
        .filter(|index| !DEFAULT_SORT_EXPORTS.contains(&object.members[*index].key.as_str()))
        .collect();
    let reordered = others.windows(2).any(|pair| pair[0] > pair[1]);
    if object.reorder(&positions) {
        change.moved = true;
        change.reordered |= reordered;
    }
    for member in &mut object.members {
        sort_exports(&mut member.value, order, change);
    }
}

/// Sorts an object by key or an array of strings by value, one level deep, in byte order.
fn sort_az(node: &mut Node) -> bool {
    match node {
        Node::Object(object) => {
            let mut order: Vec<usize> = (0..object.members.len()).collect();
            order.sort_by(|a, b| object.members[*a].key.cmp(&object.members[*b].key));
            object.reorder(&order)
        }
        Node::Array(array) => {
            let Some(values) = array.items.iter().map(string_value).collect::<Option<Vec<&str>>>() else {
                return false;
            };
            let mut order: Vec<usize> = (0..values.len()).collect();
            order.sort_by(|a, b| values[*a].cmp(values[*b]));
            if order.iter().enumerate().all(|(position, index)| position == *index) {
                return false;
            }
            let mut taken: Vec<Option<Node>> = array.items.drain(..).map(Some).collect();
            array.items = order.iter().filter_map(|index| taken[*index].take()).collect();
            true
        }
        Node::Scalar { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(source: &str, options: &FormatOptions) -> Formatted {
        format_package_json(source, options).unwrap()
    }

    fn defaults(source: &str) -> Formatted {
        format(source, &FormatOptions::default())
    }

    fn assert_idempotent(source: &str, options: &FormatOptions) {
        let once = format(source, options);
        let twice = format(&once.output, options);
        assert_eq!(twice.issues, [], "formatting the output again finds issues");
        assert_eq!(twice.output, once.output);
    }

    #[test]
    fn a_formatted_file_is_unchanged() {
        let source = "{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\",\n  \"private\": true\n}\n";
        let formatted = defaults(source);
        assert_eq!(formatted.issues, []);
        assert_eq!(formatted.output, source);
    }

    #[test]
    fn sort_first_then_byte_order_keeps_the_layout() {
        let source =
            "{\n    \"version\": \"1.0.0\",\n    \"scripts\": {\"b\": \"x\", \"a\": \"y\"},\n    \"name\": \"a\"\n}";
        let formatted = defaults(source);
        assert_eq!(
            formatted.issues,
            [FormatIssue::KeyOrder, FormatIssue::SortAz("scripts".to_owned())]
        );
        assert_eq!(
            formatted.output,
            "{\n    \"name\": \"a\",\n    \"version\": \"1.0.0\",\n    \"scripts\": {\"a\": \"y\", \"b\": \"x\"}\n}"
        );
        assert_idempotent(source, &FormatOptions::default());
    }

    #[test]
    fn without_sort_packages_the_rest_keeps_its_order() {
        let options = FormatOptions::default().with_sort_packages(false);
        let formatted = format(r#"{"z":1,"version":"1","a":2,"name":"n"}"#, &options);
        assert_eq!(formatted.output, r#"{"name":"n","version":"1","z":1,"a":2}"#);
    }

    #[test]
    fn empty_sort_first_and_no_sort_packages_turn_the_step_off() {
        let options = FormatOptions::default()
            .with_sort_first(Vec::new())
            .with_sort_packages(false);
        assert_eq!(format(r#"{"z":1,"a":2}"#, &options).issues, []);
    }

    #[test]
    fn bugs_collapse_only_when_nothing_is_lost() {
        let options = FormatOptions::default().with_format_bugs(true);
        let formatted = format(r#"{"bugs":{"url":"https://x/issues"}}"#, &options);
        assert_eq!(formatted.issues, [FormatIssue::Bugs]);
        assert_eq!(formatted.output, r#"{"bugs":"https://x/issues"}"#);
        let with_email = r#"{"bugs":{"url":"https://x","email":"a@b"}}"#;
        assert_eq!(format(with_email, &options).issues, []);
        assert_eq!(format(r#"{"bugs":"https://x"}"#, &options).issues, []);
        assert_eq!(
            defaults(r#"{"bugs":{"url":"https://x/issues"}}"#).issues,
            [],
            "off by default"
        );
    }

    #[test]
    fn repository_collapses_to_the_github_shorthand() {
        let options = FormatOptions::default().with_format_repository(true);
        let formatted = format(
            r#"{"repository":{"type":"git","url":"git+https://github.com/o/r.git"}}"#,
            &options,
        );
        assert_eq!(formatted.issues, [FormatIssue::Repository]);
        assert_eq!(formatted.output, r#"{"repository":"o/r.git"}"#);
        let gitlab = format(
            r#"{"repository":{"type":"git","url":"https://gitlab.com/o/r"}}"#,
            &options,
        );
        assert_eq!(gitlab.output, r#"{"repository":"https://gitlab.com/o/r"}"#);
        let directory = r#"{"repository":{"type":"git","url":"https://github.com/o/r","directory":"packages/a"}}"#;
        assert_eq!(format(directory, &options).issues, []);
        assert_eq!(format(r#"{"repository":{"type":"git"}}"#, &options).issues, []);
        let untyped = format(r#"{"repository":{"url":"https://github.com/o/r"}}"#, &options);
        assert_eq!(untyped.output, r#"{"repository":"o/r"}"#);
        for other in ["svn", "hg", ""] {
            let source = format!(r#"{{"repository":{{"type":"{other}","url":"https://github.com/o/r"}}}}"#);
            assert_eq!(format(&source, &options).issues, [], "type {other:?}");
        }
        let not_a_string = r#"{"repository":{"type":1,"url":"https://github.com/o/r"}}"#;
        assert_eq!(format(not_a_string, &options).issues, []);
    }

    #[test]
    fn exports_conditions_move_only_types_first_and_default_last() {
        let source = r#"{"exports":{".":{"default":"./a.js","import":"./a.mjs","types":"./a.d.ts","custom":"./c.js"},"./b":{"require":"./b.cjs","import":"./b.mjs"}}}"#;
        let formatted = defaults(source);
        assert_eq!(formatted.issues, [FormatIssue::Exports]);
        assert!(formatted.issues.iter().all(FormatIssue::is_safe));
        assert_eq!(
            formatted.output,
            r#"{"exports":{".":{"types":"./a.d.ts","import":"./a.mjs","custom":"./c.js","default":"./a.js"},"./b":{"require":"./b.cjs","import":"./b.mjs"}}}"#
        );
        assert_idempotent(source, &FormatOptions::default());
    }

    #[test]
    fn exports_conditions_whose_order_decides_resolution_stay_in_place() {
        for source in [
            r#"{"exports":{"development":"./dev.js","import":"./a.mjs","default":"./a.js"}}"#,
            r#"{"exports":{"workerd":"./worker.js","node":"./node.js","default":"./a.js"}}"#,
            r#"{"exports":{"import":"./a.mjs","module":"./a.esm.js","default":"./a.js"}}"#,
            r#"{"exports":{".":{"browser":"./b.js","node":"./n.js"}}}"#,
        ] {
            let formatted = defaults(source);
            assert_eq!(formatted.issues, [], "{source}");
            assert_eq!(formatted.output, source);
        }
        let formatted = defaults(r#"{"exports":{"default":"./a.js","development":"./dev.js","types":"./a.d.ts"}}"#);
        assert_eq!(
            formatted.output,
            r#"{"exports":{"types":"./a.d.ts","development":"./dev.js","default":"./a.js"}}"#
        );
    }

    #[test]
    fn a_longer_sort_exports_reorders_the_other_conditions_unsafely() {
        let options = FormatOptions::default().with_sort_exports(
            ["types", "node", "browser", "module", "import", "default"]
                .map(str::to_owned)
                .to_vec(),
        );
        let source = r#"{"exports":{"import":"./a.mjs","module":"./a.esm.js","types":"./a.d.ts"}}"#;
        let formatted = format(source, &options);
        assert_eq!(formatted.issues, [FormatIssue::ExportsReordered]);
        assert!(!formatted.issues[0].is_safe());
        assert_eq!(
            formatted.output,
            r#"{"exports":{"types":"./a.d.ts","module":"./a.esm.js","import":"./a.mjs"}}"#
        );
        assert_idempotent(source, &options);
        let only_types = format(r#"{"exports":{"import":"./a.mjs","types":"./a.d.ts"}}"#, &options);
        assert_eq!(
            only_types.issues,
            [FormatIssue::Exports],
            "moving `types` alone is safe"
        );
    }

    #[test]
    fn sort_az_sorts_arrays_of_strings_and_leaves_mixed_arrays() {
        let formatted = defaults(r#"{"keywords":["b","a"],"bin":{"z":"1","a":"2"}}"#);
        assert_eq!(
            formatted.issues,
            [
                FormatIssue::KeyOrder,
                FormatIssue::SortAz("bin".to_owned()),
                FormatIssue::SortAz("keywords".to_owned())
            ]
        );
        assert_eq!(formatted.output, r#"{"bin":{"a":"2","z":"1"},"keywords":["a","b"]}"#);
        assert_eq!(defaults(r#"{"keywords":["b",1,"a"]}"#).issues, []);
    }

    #[test]
    fn sort_az_uses_byte_order_and_nested_paths() {
        let options = FormatOptions::default()
            .with_sort_az(vec!["a/b".to_owned()])
            .with_sort_packages(false);
        let formatted = format(r#"{"a":{"b":{"x":1,"B":2,"@s":3}}}"#, &options);
        assert_eq!(formatted.output, r#"{"a":{"b":{"@s":3,"B":2,"x":1}}}"#);
    }

    #[test]
    fn indent_lays_the_file_out_like_json_stringify() {
        let options = FormatOptions::default().with_indent(Some("\t".to_owned()));
        let source = "{\"name\":\"a\",\"files\":[\"dist\"],\"empty\":{},\"none\":[],\"n\":1.50,\"s\":\"\\u0041\"}";
        let formatted = format(source, &options);
        assert_eq!(formatted.issues, [FormatIssue::KeyOrder, FormatIssue::Layout]);
        assert_eq!(
            formatted.output,
            "{\n\t\"name\": \"a\",\n\t\"empty\": {},\n\t\"files\": [\n\t\t\"dist\"\n\t],\n\t\"n\": 1.50,\n\t\"none\": [],\n\t\"s\": \"\\u0041\"\n}\n"
        );
        assert_idempotent(source, &options);
    }

    #[test]
    fn indent_keeps_crlf_and_the_bom() {
        let options = FormatOptions::default().with_indent(Some("  ".to_owned()));
        let source = "\u{feff}{\r\n    \"name\": \"a\"\r\n}\r\n";
        let formatted = format(source, &options);
        assert_eq!(formatted.output, "\u{feff}{\r\n  \"name\": \"a\"\r\n}\r\n");
        assert_eq!(
            format("{\n  \"name\": \"a\"\n}\n", &options).issues,
            [],
            "an indented file is formatted"
        );
    }

    #[test]
    fn indent_keeps_the_layout_of_a_file_that_is_not_strict_json() {
        let options = FormatOptions::default().with_indent(Some("  ".to_owned()));
        for source in [
            "{\n    // the package\n    \"name\": \"a\"\n}\n",
            "{\n    \"name\": \"a\" /* the package */\n}\n",
            "{\n    \"name\": \"a\",\n}\n",
            "{\n    'name': 'a'\n}\n",
            "{\n    \"n\": 0x1F\n}\n",
            "// header\n{\n    \"name\": \"a\"\n}\n",
        ] {
            let formatted = format(source, &options);
            assert_eq!(formatted.issues, [], "{source}");
            assert_eq!(formatted.output, source);
        }
        let reordered = format("{\n    \"version\": \"1\",\n    \"name\": \"a\",\n}\n", &options);
        assert_eq!(reordered.issues, [FormatIssue::KeyOrder], "the other steps still run");
        assert_eq!(
            reordered.output,
            "{\n    \"name\": \"a\",\n    \"version\": \"1\",\n}\n"
        );
    }

    #[test]
    fn a_root_that_is_not_an_object_is_an_error() {
        assert!(format_package_json("[]", &FormatOptions::default()).is_err());
        assert!(format_package_json("{", &FormatOptions::default()).is_err());
    }

    #[test]
    fn minimal_edit_covers_only_the_difference() {
        assert_eq!(minimal_edit("abc", "abc"), None);
        assert_eq!(minimal_edit("a1c", "a2c"), Some(TextEdit::new(1, 2, "2")));
        assert_eq!(minimal_edit("ab", "abab"), Some(TextEdit::new(2, 2, "ab")));
        assert_eq!(minimal_edit("é", "è"), Some(TextEdit::new(0, 2, "è")));
        for (before, after) in [("x\"é\"y", "x\"è\"y"), ("aaa", "a"), ("", "x")] {
            let edit = minimal_edit(before, after).unwrap();
            let applied = skif_core::apply_text_edits(before, &[edit]).unwrap();
            assert_eq!(applied, after);
        }
    }
}
