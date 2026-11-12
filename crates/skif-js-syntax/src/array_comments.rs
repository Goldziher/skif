//! The comments written next to the elements of array literals and the properties of object literals, for a tool
//! that carries them over into another format (`skif migrate` turns the comment on a knip ignore entry, on the group
//! of entries it heads, or on the list itself into the `reason` of each entry).

use skif_core::HashMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrayExpression, ArrayExpressionElement, Comment, Declaration, Expression, ObjectExpression, ObjectPropertyKind,
    Program, Statement,
};
use oxc_ast_visit::Visit;
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::{GetSpan, SourceType};

use crate::finder::static_string;

/// The directives a comment can start with, each a whole word: `// eslint-disable-next-line no-console` is one,
/// `// eslint-plugin-foo is loaded by name` is not.
const DIRECTIVES: &[&str] = &[
    "eslint-disable",
    "eslint-disable-next-line",
    "eslint-disable-line",
    "eslint-enable",
    "eslint-env",
    "@ts-ignore",
    "@ts-expect-error",
    "@ts-nocheck",
    "@ts-check",
    "istanbul ignore",
    "c8 ignore",
    "prettier-ignore",
    "biome-ignore",
    "biome-ignore-all",
    "biome-ignore-start",
    "biome-ignore-end",
    "oxlint-disable",
    "oxlint-disable-next-line",
    "oxlint-disable-line",
    "oxlint-enable",
];

/// Whether the comment `text` (`block` for a `/* */` comment) is a tool directive rather than an explanation: one of
/// [`DIRECTIVES`] followed by the end of the text, a space or a colon, or the block comment `ESLint` reads as rule
/// configuration, `/* eslint <rule>: <setting> */`.
fn is_directive(text: &str, block: bool) -> bool {
    let whole_word = |rest: &str| {
        rest.chars()
            .next()
            .is_none_or(|next| next.is_whitespace() || next == ':')
    };
    DIRECTIVES
        .iter()
        .any(|directive| text.strip_prefix(directive).is_some_and(whole_word))
        || (block && is_eslint_rule_config(text))
}

/// Whether `text` is `eslint` followed by a rule name and a colon, as in `eslint no-console: "off"` or
/// `eslint @typescript-eslint/no-explicit-any: 0`.
fn is_eslint_rule_config(text: &str) -> bool {
    let Some(rest) = text
        .strip_prefix("eslint")
        .filter(|rest| rest.starts_with(char::is_whitespace))
    else {
        return false;
    };
    let Some((rule, _)) = rest.trim_start().split_once(':') else {
        return false;
    };
    let rule = rule.trim_end();
    !rule.is_empty()
        && rule
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '/' | '@'))
}

/// One step of the path from an outermost object or array literal to an array literal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PathSegment {
    /// The value of the property with this static key.
    Key(String),
    /// The element at this position.
    Index(usize),
}

/// The comment written next to one element of an array literal: on the lines directly above it, or after it on the
/// line where it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ElementComment {
    /// The path to the array from the outermost literal that contains it.
    pub array: Vec<PathSegment>,
    /// The position of the element in the array literal; a spread counts as one element.
    pub index: usize,
    /// Whether the element is a spread or comes after one. An evaluator that expands spreads puts such an element at
    /// a position that `index` does not give, so pairing by position is only sound when this is `false`.
    pub after_spread: bool,
    /// The element when it is a string (its value) or a regular expression literal (its pattern).
    pub value: Option<String>,
    /// The comment text without its delimiters, its lines joined by single spaces. Comments both above and after
    /// the element are joined in source order.
    pub text: String,
    /// The element has no comment of its own and `text` is its group heading: the comments above the nearest element
    /// before it in the same array that has comments above it. A list written as groups, each under one comment
    /// line, explains every element of a group with that line.
    pub heading: bool,
}

impl ElementComment {
    /// The comment `text` on the element `index` of the array at `array`, whose value is `value`.
    #[must_use]
    pub fn new(array: Vec<PathSegment>, index: usize, value: Option<String>, text: impl Into<String>) -> Self {
        Self {
            array,
            index,
            after_spread: false,
            value,
            text: text.into(),
            heading: false,
        }
    }

    /// This comment, recording that its element is a spread or comes after one.
    #[must_use]
    pub fn after_spread(mut self) -> Self {
        self.after_spread = true;
        self
    }

    /// This comment, recording that it is the group heading of its element rather than a comment of its own.
    #[must_use]
    pub fn under_heading(mut self) -> Self {
        self.heading = true;
        self
    }
}

/// The comment written next to a property of an object literal: on the lines directly above it, after it on the line
/// where it ends, or, when its value is an array literal, after the opening bracket on the bracket's line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PropertyComment {
    /// The path from the outermost literal to the property's value; its last segment is the property's key.
    pub path: Vec<PathSegment>,
    /// The comment text without its delimiters, its lines joined by single spaces, joined in source order.
    pub text: String,
}

impl PropertyComment {
    /// The comment `text` on the property at `path`.
    #[must_use]
    pub fn new(path: Vec<PathSegment>, text: impl Into<String>) -> Self {
        Self {
            path,
            text: text.into(),
        }
    }
}

/// The comments of a configuration file's literals, from [`extract_config_comments`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConfigComments {
    /// The comments on array elements, including the group heading of each element without a comment of its own.
    pub elements: Vec<ElementComment>,
    /// The comments on object properties.
    pub properties: Vec<PropertyComment>,
}

/// The comments next to the elements of the array literals of `source`, a JavaScript or TypeScript file whose
/// extension (in `path`) selects the dialect. Paths start at each outermost object or array literal, so the object
/// a config file exports, directly or through a constant or a call such as `defineConfig({ ... })`, is the root. A
/// property or element whose value is the name of a top-level `const` bound to an object or array literal is followed
/// into that literal, so `const deps = [...]; export default { ignoreDependencies: deps }` puts the comments of `deps`
/// under `ignoreDependencies` (they are also reported under `deps` as a root of its own). A spread is not followed. A
/// comment that is a tool directive (`// eslint-disable-next-line`, `// @ts-expect-error`) is left out.
///
/// The elements a comment only heads (see [`ElementComment::heading`]) are left out; [`extract_config_comments`] has
/// them, and the comments on properties.
///
/// `None` when the file does not parse; nothing is executed.
#[must_use]
pub fn extract_array_comments(source: &str, path: &Path) -> Option<Vec<ElementComment>> {
    let mut comments = extract_config_comments(source, path)?.elements;
    comments.retain(|comment| !comment.heading);
    Some(comments)
}

/// The comments of `source` that [`extract_array_comments`] reads, with an [`ElementComment`] under a heading for
/// every element whose only explanation is the comment above an earlier element of its array, and a
/// [`PropertyComment`] for every property with a static key that has a comment. Properties are walked as elements
/// are: a property whose value names a top-level `const` literal is followed into it.
///
/// `None` when the file does not parse; nothing is executed.
#[must_use]
pub fn extract_config_comments(source: &str, path: &Path) -> Option<ConfigComments> {
    let source_type = SourceType::from_path(path).ok()?;
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return None;
    }
    let mut roots = Roots {
        source,
        comments: &ret.program.comments,
        constants: constants(&ret.program),
        following: Vec::new(),
        out: ConfigComments::default(),
    };
    roots.visit_program(&ret.program);
    Some(roots.out)
}

/// The object and array literals bound to the top-level constants of `program`, exported or not, by name.
fn constants<'s, 'a>(program: &'s Program<'a>) -> HashMap<&'s str, &'s Expression<'a>> {
    program
        .body
        .iter()
        .filter_map(|statement| match statement {
            Statement::VariableDeclaration(declaration) => Some(&**declaration),
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::VariableDeclaration(declaration) => Some(&**declaration),
                _ => None,
            },
            _ => None,
        })
        .filter(|declaration| declaration.kind.is_const())
        .flat_map(|declaration| declaration.declarations.iter())
        .filter_map(|declarator| {
            let name = declarator.id.get_binding_identifier()?.name.as_str();
            let init = declarator.init.as_ref()?;
            matches!(
                init.get_inner_expression(),
                Expression::ObjectExpression(_) | Expression::ArrayExpression(_)
            )
            .then_some((name, init))
        })
        .collect()
}

/// Walks the outermost literals; the literals inside them are walked by [`Roots::value`], which keeps the path.
struct Roots<'s, 'a> {
    source: &'s str,
    comments: &'s [Comment],
    /// The literals a property or element can name, from [`constants`].
    constants: HashMap<&'s str, &'s Expression<'a>>,
    /// The constants being followed, innermost last, so that a cycle (`const a = [b]; const b = [a];`) ends.
    following: Vec<&'s str>,
    out: ConfigComments,
}

impl<'a> Visit<'a> for Roots<'_, '_> {
    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        self.object(it, &mut Vec::new());
    }

    fn visit_array_expression(&mut self, it: &ArrayExpression<'a>) {
        self.array(it, &mut Vec::new());
    }
}

impl Roots<'_, '_> {
    fn value(&mut self, expression: &Expression<'_>, path: &mut Vec<PathSegment>) {
        match expression.get_inner_expression() {
            Expression::ObjectExpression(object) => self.object(object, path),
            Expression::ArrayExpression(array) => self.array(array, path),
            Expression::Identifier(identifier) => {
                let Some((&name, &bound)) = self.constants.get_key_value(identifier.name.as_str()) else {
                    return;
                };
                if !self.following.contains(&name) {
                    self.following.push(name);
                    self.value(bound, path);
                    self.following.pop();
                }
            }
            _ => {}
        }
    }

    fn object(&mut self, object: &ObjectExpression<'_>, path: &mut Vec<PathSegment>) {
        let spans: Vec<(u32, u32)> = object
            .properties
            .iter()
            .map(|property| (property.span().start, property.span().end))
            .collect();
        for (index, kind) in object.properties.iter().enumerate() {
            let ObjectPropertyKind::ObjectProperty(property) = kind else {
                continue;
            };
            let Some(key) = property.key.static_name() else {
                continue;
            };
            path.push(PathSegment::Key(crate::js_string(key.as_js_str())));
            let (previous_end, next_start) = neighbours(&spans, index, object.span.start + 1, object.span.end);
            let (start, end) = spans[index];
            let mut texts = self.leading(previous_end, start);
            texts.extend(self.after_opening_bracket(&property.value));
            texts.extend(self.trailing(end, next_start));
            if !texts.is_empty() {
                self.out
                    .properties
                    .push(PropertyComment::new(path.clone(), texts.join(" ")));
            }
            self.value(&property.value, path);
            path.pop();
        }
    }

    /// The comments after the opening bracket of `value`, when it is an array literal, on the bracket's line.
    fn after_opening_bracket(&self, value: &Expression<'_>) -> Vec<String> {
        let Expression::ArrayExpression(array) = value.get_inner_expression() else {
            return Vec::new();
        };
        let open = array.span.start + 1;
        let first = array
            .elements
            .first()
            .map_or(array.span.end, |element| element.span().start);
        self.comments
            .iter()
            .filter(|comment| comment.span.start >= open && comment.span.end <= first)
            .filter(|comment| !self.has_newline(open, comment.span.start))
            .filter_map(|comment| self.comment_text(comment))
            .collect()
    }

    fn array(&mut self, array: &ArrayExpression<'_>, path: &mut Vec<PathSegment>) {
        let spans: Vec<(u32, u32)> = array
            .elements
            .iter()
            .map(|element| (element.span().start, element.span().end))
            .collect();
        let mut after_spread = false;
        let mut heading: Option<String> = None;
        for (index, element) in array.elements.iter().enumerate() {
            after_spread |= matches!(element, ArrayExpressionElement::SpreadElement(_));
            let (previous_end, next_start) = neighbours(&spans, index, array.span.start + 1, array.span.end);
            let (start, end) = spans[index];
            let leading = self.leading(previous_end, start);
            if !leading.is_empty() {
                heading = Some(leading.join(" "));
            }
            let mut own = leading;
            own.extend(self.trailing(end, next_start));
            let comment = if own.is_empty() {
                heading.clone().map(|text| (text, true))
            } else {
                Some((own.join(" "), false))
            };
            if let Some((text, under_heading)) = comment {
                let value = element
                    .as_expression()
                    .and_then(|expression| match expression.get_inner_expression() {
                        Expression::RegExpLiteral(regex) => Some(regex.regex.pattern.text.as_str().to_owned()),
                        other => static_string(other),
                    });
                let mut comment = ElementComment::new(path.clone(), index, value, text);
                if under_heading {
                    comment = comment.under_heading();
                }
                if after_spread {
                    comment = comment.after_spread();
                }
                self.out.elements.push(comment);
            }
            if let Some(expression) = element.as_expression() {
                path.push(PathSegment::Index(index));
                self.value(expression, path);
                path.pop();
            }
        }
    }

    /// The texts of the comments on the lines above the element or property that starts at `start`, after the end of
    /// the previous one (or the opening bracket or brace, `previous_end`), with no blank line in between. A comment on
    /// the line where the previous one ends belongs to that one, and one on the line of the opening bracket to none.
    fn leading(&self, previous_end: u32, start: u32) -> Vec<String> {
        let mut texts: Vec<String> = Vec::new();
        let leading: Vec<&Comment> = self
            .comments
            .iter()
            .filter(|comment| comment.span.start >= previous_end && comment.span.end <= start)
            .filter(|comment| self.has_newline(previous_end, comment.span.start))
            .collect();
        // Keep the run of comments that ends directly above the element: walk back while only one line break
        // separates a comment from what follows it.
        let mut boundary = start;
        let mut kept: Vec<&Comment> = Vec::new();
        for comment in leading.into_iter().rev() {
            if self.line_breaks(comment.span.end, boundary) > 1 {
                break;
            }
            kept.push(comment);
            boundary = comment.span.start;
        }
        for comment in kept.iter().rev() {
            texts.extend(self.comment_text(comment));
        }
        texts
    }

    /// The texts of the comments after the element or property that ends at `end`, on the line where it ends, before
    /// the start of the next one (or the closing bracket or brace, `next_start`).
    fn trailing(&self, end: u32, next_start: u32) -> Vec<String> {
        self.comments
            .iter()
            .filter(|comment| comment.span.start >= end && comment.span.end <= next_start)
            .filter(|comment| !self.has_newline(end, comment.span.start))
            .filter_map(|comment| self.comment_text(comment))
            .collect()
    }

    fn slice(&self, from: u32, to: u32) -> &str {
        self.source.get(from as usize..to as usize).unwrap_or_default()
    }

    fn line_breaks(&self, from: u32, to: u32) -> usize {
        self.slice(from, to).matches('\n').count()
    }

    fn has_newline(&self, from: u32, to: u32) -> bool {
        self.line_breaks(from, to) > 0
    }

    /// The text of `comment` without its delimiters and with its lines joined, or `None` for an empty comment or a
    /// tool directive.
    fn comment_text(&self, comment: &Comment) -> Option<String> {
        let raw = self.slice(comment.span.start, comment.span.end);
        let body = raw.strip_prefix("//").unwrap_or_else(|| {
            raw.strip_prefix("/*")
                .map_or(raw, |inner| inner.strip_suffix("*/").unwrap_or(inner))
        });
        let text = body
            .lines()
            .map(|line| line.trim().trim_start_matches('*').trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty() && !is_directive(&text, comment.is_block())).then_some(text)
    }
}

/// The end of the item before `index` (or `open`, after the opening bracket or brace) and the start of the item after
/// it (or `close`, the end of the literal), from the spans of the items of one literal.
fn neighbours(spans: &[(u32, u32)], index: usize, open: u32, close: u32) -> (u32, u32) {
    let previous_end = index
        .checked_sub(1)
        .and_then(|previous| spans.get(previous))
        .map_or(open, |(_, end)| *end);
    let next_start = spans.get(index + 1).map_or(close, |(start, _)| *start);
    (previous_end, next_start)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn comments(source: &str) -> Vec<ElementComment> {
        extract_array_comments(source, Path::new("knip.ts")).unwrap()
    }

    fn key(name: &str) -> PathSegment {
        PathSegment::Key(name.to_owned())
    }

    #[test]
    fn a_comment_above_an_element_is_attached_to_it() {
        let found = comments(
            "export default {\n  workspaces: {\n    'packages/app': {\n      ignoreDependencies: [\n        // false positive: loaded by the bundler from a config string\n        'some-loader',\n      ],\n    },\n  },\n};\n",
        );
        assert_eq!(
            found,
            [ElementComment::new(
                vec![key("workspaces"), key("packages/app"), key("ignoreDependencies")],
                0,
                Some("some-loader".to_owned()),
                "false positive: loaded by the bundler from a config string",
            )]
        );
    }

    #[test]
    fn a_trailing_comment_belongs_to_the_element_it_follows() {
        let found = comments(
            "const config = {\n  ignoreBinaries: [\n    'docker', // installed on the CI image\n    'make',\n    /^tsc/, /* the compiler */\n  ],\n};\nexport default config;\n",
        );
        assert_eq!(
            found,
            [
                ElementComment::new(
                    vec![key("ignoreBinaries")],
                    0,
                    Some("docker".to_owned()),
                    "installed on the CI image"
                ),
                ElementComment::new(vec![key("ignoreBinaries")], 2, Some("^tsc".to_owned()), "the compiler"),
            ]
        );
    }

    #[test]
    fn a_comment_separated_by_a_blank_line_and_directives_are_left_out() {
        let found = comments(
            "export default defineConfig({\n  ignore: [\n    // about the whole list\n\n    'a',\n    // eslint-disable-next-line\n    'b',\n    /**\n     * generated\n     * by the build\n     */\n    'c',\n  ],\n});\n",
        );
        assert_eq!(
            found,
            [ElementComment::new(
                vec![key("ignore")],
                2,
                Some("c".to_owned()),
                "generated by the build"
            )]
        );
    }

    #[test]
    fn every_directive_form_is_left_out() {
        let found = comments(
            "export default {\n  ignore: [\n    // eslint-disable-next-line no-console -- noisy\n    'a',\n    'b', // eslint-disable-line\n    /* eslint-disable */\n    'c',\n    /* eslint-enable */\n    'd',\n    /* eslint-env node */\n    'e',\n    /* eslint no-console: \"off\" */\n    'f',\n    /* eslint @typescript-eslint/no-explicit-any: 0 */\n    'g',\n    // @ts-ignore\n    'h',\n    // @ts-expect-error: the types lag\n    'i',\n    // @ts-nocheck\n    'j',\n    // @ts-check\n    'k',\n    /* istanbul ignore next */\n    'l',\n    /* c8 ignore next */\n    'm',\n    // prettier-ignore\n    'n',\n    // biome-ignore lint/style: generated\n    'o',\n    // oxlint-disable-next-line no-unused-vars\n    'p',\n  ],\n};\n",
        );
        assert_eq!(found, []);
    }

    #[test]
    fn a_reason_that_starts_with_a_package_or_tool_name_is_kept() {
        let found = comments(
            "export default {\n  ignoreDependencies: [\n    // eslint-plugin-foo is loaded by name\n    'eslint-plugin-foo',\n    // @ts-rest/core is pulled in by codegen\n    '@ts-rest/core',\n    // eslint config resolves it\n    'eslint-config-acme',\n    /* eslint config resolves this one too */\n    'eslint-config-other',\n    // eslint: the line form never configures a rule\n    'eslint-formatter-x',\n    // oxlint-plugin-bar is loaded by name\n    'oxlint-plugin-bar',\n    // c8 is run by the coverage job\n    'c8',\n  ],\n};\n",
        );
        let reasons: Vec<(Option<&str>, &str)> = found
            .iter()
            .map(|comment| (comment.value.as_deref(), comment.text.as_str()))
            .collect();
        assert_eq!(
            reasons,
            [
                (Some("eslint-plugin-foo"), "eslint-plugin-foo is loaded by name"),
                (Some("@ts-rest/core"), "@ts-rest/core is pulled in by codegen"),
                (Some("eslint-config-acme"), "eslint config resolves it"),
                (Some("eslint-config-other"), "eslint config resolves this one too"),
                (
                    Some("eslint-formatter-x"),
                    "eslint: the line form never configures a rule"
                ),
                (Some("oxlint-plugin-bar"), "oxlint-plugin-bar is loaded by name"),
                (Some("c8"), "c8 is run by the coverage job"),
            ]
        );
    }

    #[test]
    fn object_elements_are_attached_by_position() {
        let found = extract_array_comments(
            "module.exports = {\n  \"versionGroups\": [\n    // react is pinned until the upgrade\n    { \"dependencies\": [\"react\"], \"isIgnored\": true }\n  ]\n}",
            Path::new("config.js"),
        )
        .unwrap();
        assert_eq!(
            found,
            [ElementComment::new(
                vec![key("versionGroups")],
                0,
                None,
                "react is pinned until the upgrade"
            )]
        );
    }

    #[test]
    fn a_spread_and_the_elements_after_it_are_marked() {
        let found = comments(
            "export default {\n  versionGroups: [\n    // first\n    { isIgnored: true },\n    ...shared, // the shared groups\n    // pinned\n    { isIgnored: true },\n  ],\n};\n",
        );
        assert_eq!(
            found,
            [
                ElementComment::new(vec![key("versionGroups")], 0, None, "first"),
                ElementComment::new(vec![key("versionGroups")], 1, None, "the shared groups").after_spread(),
                ElementComment::new(vec![key("versionGroups")], 2, None, "pinned").after_spread(),
            ]
        );
    }

    #[test]
    fn a_list_behind_a_constant_is_followed_from_the_key_that_names_it() {
        let found = comments(
            "const deps = [\n  // loaded by the bundler\n  'some-loader',\n];\nexport const shared = { ignoreBinaries: ['make' /* from the image */] };\nconst loop = [loop];\nexport default {\n  ignoreDependencies: deps,\n  workspaces: { 'packages/app': shared },\n  ignore: loop,\n};\n",
        );
        let loader = |array: Vec<PathSegment>| {
            ElementComment::new(array, 0, Some("some-loader".to_owned()), "loaded by the bundler")
        };
        let make = |array: Vec<PathSegment>| ElementComment::new(array, 0, Some("make".to_owned()), "from the image");
        assert_eq!(
            found,
            [
                loader(vec![]),
                make(vec![key("ignoreBinaries")]),
                loader(vec![key("ignoreDependencies")]),
                make(vec![key("workspaces"), key("packages/app"), key("ignoreBinaries")]),
            ]
        );
    }

    #[test]
    fn a_file_that_does_not_parse_has_no_comments() {
        assert_eq!(extract_array_comments("export default {", Path::new("knip.ts")), None);
        assert_eq!(extract_config_comments("export default {", Path::new("knip.ts")), None);
    }

    fn config_comments(source: &str) -> ConfigComments {
        extract_config_comments(source, Path::new("knip.ts")).unwrap()
    }

    #[test]
    fn a_comment_above_a_group_heads_the_elements_below_it_without_their_own() {
        let found = config_comments(
            "export default {\n  ignoreDependencies: [\n    'plain',\n    // loaded by the bundler\n    'some-loader',\n    'other-loader',\n    'left-pad', // kept for the legacy build\n    'after-trailing',\n    // types only\n    '@demo/types',\n    ...shared,\n    '@demo/runtime',\n  ],\n};\n",
        );
        let array = || vec![key("ignoreDependencies")];
        let value = |name: &str| Some(name.to_owned());
        assert_eq!(
            found.elements,
            [
                ElementComment::new(array(), 1, value("some-loader"), "loaded by the bundler"),
                ElementComment::new(array(), 2, value("other-loader"), "loaded by the bundler").under_heading(),
                ElementComment::new(array(), 3, value("left-pad"), "kept for the legacy build"),
                ElementComment::new(array(), 4, value("after-trailing"), "loaded by the bundler").under_heading(),
                ElementComment::new(array(), 5, value("@demo/types"), "types only"),
                ElementComment::new(array(), 6, None, "types only")
                    .under_heading()
                    .after_spread(),
                ElementComment::new(array(), 7, value("@demo/runtime"), "types only")
                    .under_heading()
                    .after_spread(),
            ]
        );
        assert_eq!(found.properties, []);
    }

    #[test]
    fn array_comments_leave_out_the_elements_under_a_heading() {
        let found = comments("export default {\n  ignore: [\n    // generated\n    'a',\n    'b',\n  ],\n};\n");
        assert_eq!(
            found,
            [ElementComment::new(
                vec![key("ignore")],
                0,
                Some("a".to_owned()),
                "generated"
            )]
        );
    }

    #[test]
    fn a_comment_on_a_property_belongs_to_its_path() {
        let found = config_comments(
            "const deps = ['b'];\nexport default {\n  // installed on the CI image\n  ignoreBinaries: ['docker', 'make'],\n  ignoreUnresolved: [ // virtual modules\n    'virtual:routes',\n  ],\n  ignoreFiles: ['a'], // generated\n  // about the next one\n\n  ignoreMembers: ['render'],\n  workspaces: {\n    'packages/app': {\n      /* kept by the host page */\n      ignoreDependencies: deps,\n    },\n  },\n};\n",
        );
        let property = |path: Vec<PathSegment>, text: &str| PropertyComment::new(path, text);
        assert_eq!(
            found.properties,
            [
                property(vec![key("ignoreBinaries")], "installed on the CI image"),
                property(vec![key("ignoreUnresolved")], "virtual modules"),
                property(vec![key("ignoreFiles")], "generated"),
                property(
                    vec![key("workspaces"), key("packages/app"), key("ignoreDependencies")],
                    "kept by the host page"
                ),
            ]
        );
        assert_eq!(found.elements, []);
    }
}
