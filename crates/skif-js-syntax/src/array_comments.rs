//! The comments written next to the elements of array literals, for a tool that carries them over into another
//! format (`skif migrate` turns the comment on a knip ignore entry into its `reason`).

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{ArrayExpression, Comment, Expression, ObjectExpression, ObjectPropertyKind};
use oxc_ast_visit::Visit;
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::{GetSpan, SourceType};

use crate::finder::static_string;

/// Comments that are tool directives rather than explanations.
const DIRECTIVES: &[&str] = &[
    "eslint-",
    "eslint ",
    "@ts-",
    "prettier-ignore",
    "biome-ignore",
    "oxlint-",
    "istanbul ",
    "c8 ",
];

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
    /// The element when it is a string (its value) or a regular expression literal (its pattern).
    pub value: Option<String>,
    /// The comment text without its delimiters, its lines joined by single spaces. Comments both above and after
    /// the element are joined in source order.
    pub text: String,
}

impl ElementComment {
    /// The comment `text` on the element `index` of the array at `array`, whose value is `value`.
    #[must_use]
    pub fn new(array: Vec<PathSegment>, index: usize, value: Option<String>, text: impl Into<String>) -> Self {
        Self {
            array,
            index,
            value,
            text: text.into(),
        }
    }
}

/// The comments next to the elements of the array literals of `source`, a JavaScript or TypeScript file whose
/// extension (in `path`) selects the dialect. Paths start at each outermost object or array literal, so the object
/// a config file exports, directly or through a constant or a call such as `defineConfig({ ... })`, is the root. A
/// comment that is a tool directive (`// eslint-disable-next-line`, `// @ts-expect-error`) is left out.
///
/// `None` when the file does not parse; nothing is executed.
#[must_use]
pub fn extract_array_comments(source: &str, path: &Path) -> Option<Vec<ElementComment>> {
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
        out: Vec::new(),
    };
    roots.visit_program(&ret.program);
    Some(roots.out)
}

/// Walks the outermost literals; the literals inside them are walked by [`Roots::value`], which keeps the path.
struct Roots<'s> {
    source: &'s str,
    comments: &'s [Comment],
    out: Vec<ElementComment>,
}

impl<'a> Visit<'a> for Roots<'_> {
    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        self.object(it, &mut Vec::new());
    }

    fn visit_array_expression(&mut self, it: &ArrayExpression<'a>) {
        self.array(it, &mut Vec::new());
    }
}

impl Roots<'_> {
    fn value(&mut self, expression: &Expression<'_>, path: &mut Vec<PathSegment>) {
        match expression.get_inner_expression() {
            Expression::ObjectExpression(object) => self.object(object, path),
            Expression::ArrayExpression(array) => self.array(array, path),
            _ => {}
        }
    }

    fn object(&mut self, object: &ObjectExpression<'_>, path: &mut Vec<PathSegment>) {
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            let Some(key) = property.key.static_name() else {
                continue;
            };
            path.push(PathSegment::Key(crate::js_string(key.as_js_str())));
            self.value(&property.value, path);
            path.pop();
        }
    }

    fn array(&mut self, array: &ArrayExpression<'_>, path: &mut Vec<PathSegment>) {
        let spans: Vec<(u32, u32)> = array
            .elements
            .iter()
            .map(|element| (element.span().start, element.span().end))
            .collect();
        for (index, element) in array.elements.iter().enumerate() {
            let previous_end = index
                .checked_sub(1)
                .and_then(|previous| spans.get(previous))
                .map_or(array.span.start + 1, |(_, end)| *end);
            let next_start = spans.get(index + 1).map_or(array.span.end, |(start, _)| *start);
            let (start, end) = spans[index];
            if let Some(text) = self.text_around(previous_end, start, end, next_start) {
                let value = element.as_expression().and_then(|expression| match expression.get_inner_expression() {
                    Expression::RegExpLiteral(regex) => Some(regex.regex.pattern.text.as_str().to_owned()),
                    other => static_string(other),
                });
                self.out.push(ElementComment::new(path.clone(), index, value, text));
            }
            if let Some(expression) = element.as_expression() {
                path.push(PathSegment::Index(index));
                self.value(expression, path);
                path.pop();
            }
        }
    }

    /// The text of the comments that belong to the element spanning `start..end`, between the end of the previous
    /// element (or the opening bracket, `previous_end`) and the start of the next one (or the closing bracket,
    /// `next_start`): those on the lines above it with no blank line in between, and those after it on the line where
    /// it ends. A comment on the line where the previous element ends belongs to that element, and one on the line of
    /// the opening bracket to none.
    fn text_around(&self, previous_end: u32, start: u32, end: u32, next_start: u32) -> Option<String> {
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
        for comment in self
            .comments
            .iter()
            .filter(|comment| comment.span.start >= end && comment.span.end <= next_start)
            .filter(|comment| !self.has_newline(end, comment.span.start))
        {
            texts.extend(self.comment_text(comment));
        }
        (!texts.is_empty()).then(|| texts.join(" "))
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
        let directive = DIRECTIVES.iter().any(|prefix| text.starts_with(prefix));
        (!text.is_empty() && !directive).then_some(text)
    }
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
    fn a_file_that_does_not_parse_has_no_comments() {
        assert_eq!(extract_array_comments("export default {", Path::new("knip.ts")), None);
    }
}
