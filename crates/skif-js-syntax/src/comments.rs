//! Facts that live in comments: triple-slash references, `JSDoc` imports, the `@jsxImportSource` pragma and the
//! test environment pragmas (`@jest-environment`, `@vitest-environment`).

use oxc_ast::ast::Program;
use oxc_span::{GetSpan, Span as OxcSpan};

use crate::collect::{Ctx, imported_kind};
use crate::model::{Import, ImportKind, ImportedKind, ImportedName, ModuleInfo};

/// Records the comment-borne facts of `program` in `info`.
///
/// Triple-slash directives, `@jsxImportSource` and the test environment pragmas only count in the comments before the
/// first statement; `JSDoc` imports count anywhere.
pub(crate) fn collect(ctx: &Ctx<'_>, program: &Program<'_>, info: &mut ModuleInfo) {
    let first_token = program
        .directives
        .first()
        .map(|d| d.span.start)
        .into_iter()
        .chain(program.body.first().map(|s| s.span().start))
        .min()
        .unwrap_or(u32::MAX);
    for comment in &program.comments {
        let Some(text) = ctx.src.get(comment.span.start as usize..comment.span.end as usize) else {
            continue;
        };
        if comment.span.end <= first_token {
            if info.jsx_import_source.is_none()
                && let Some((value, start)) = jsx_import_source(text)
            {
                let start = comment.span.start + start;
                let len = u32::try_from(value.len()).unwrap_or(0);
                info.jsx_import_source_span = Some(ctx.span(OxcSpan::new(start, start + len)));
                info.jsx_import_source = Some(value);
            }
            if let Some(import) = triple_slash_reference(ctx, text, comment.span) {
                info.imports.push(import);
            }
            for (specifier, start, len) in environment_pragmas(text) {
                let start = comment.span.start + start;
                info.imports.push(Import {
                    specifier,
                    kind: ImportKind::SideEffect,
                    names: Vec::new(),
                    span: ctx.span(OxcSpan::new(start, start + len)),
                });
            }
        }
        if text.starts_with("/**") {
            let flat = flatten(text);
            let type_expressions = tag_type_expressions(&flat);
            let import_types = type_expressions
                .iter()
                .flat_map(|expression| jsdoc_import_types(expression));
            for (specifier, names) in jsdoc_imports(&flat).into_iter().chain(import_types) {
                info.imports.push(Import {
                    specifier,
                    kind: ImportKind::TypeOnly,
                    names,
                    span: ctx.span(comment.span),
                });
            }
        }
    }
}

/// Value of a `@jsxImportSource` pragma and its offset within `comment`.
fn jsx_import_source(comment: &str) -> Option<(String, u32)> {
    const PRAGMA: &str = "@jsxImportSource";
    let after = comment.find(PRAGMA)? + PRAGMA.len();
    let rest = &comment[after..];
    let trimmed = rest.trim_start();
    let value: String = trimmed
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '*')
        .collect();
    let offset = u32::try_from(after + (rest.len() - trimmed.len())).ok()?;
    (!value.is_empty()).then_some((value, offset))
}

/// The environments that `@jest-environment <name>` and `@vitest-environment <name>` pragmas select, as the module the
/// test runner loads for them, each with the offset and length of the pragma within `comment`. `node` needs no
/// module; a path stays a path. Jest's `jsdom` is `jest-environment-jsdom` and Vitest's `edge-runtime` is
/// `@edge-runtime/vm`; any other name is the package it names, as knip reads it.
fn environment_pragmas(comment: &str) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    for (tool, pragma) in [("jest", "@jest-environment"), ("vitest", "@vitest-environment")] {
        let mut from = 0;
        while let Some(found) = comment[from..].find(pragma) {
            let at = from + found;
            from = at + pragma.len();
            let rest = &comment[from..];
            let value = rest.trim_start_matches([' ', '\t']);
            if value.len() == rest.len() {
                continue;
            }
            let name: String = value
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '/' | '-'))
                .collect();
            if name.is_empty() || name == "node" {
                continue;
            }
            let end = from + (rest.len() - value.len()) + name.len();
            let specifier = if tool == "jest" && name == "jsdom" {
                "jest-environment-jsdom".to_owned()
            } else if tool == "vitest" && name == "edge-runtime" {
                "@edge-runtime/vm".to_owned()
            } else {
                name
            };
            if let (Ok(start), Ok(len)) = (u32::try_from(at), u32::try_from(end - at)) {
                out.push((specifier, start, len));
            }
        }
    }
    out.sort_by_key(|(_, start, _)| *start);
    out
}

fn attribute(tag: &str, key: &str) -> Option<String> {
    let rest = tag[tag.find(&format!("{key}="))? + key.len() + 1..].trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let body = &rest[1..];
    Some(body[..body.find(quote)?].to_string())
}

/// `/// <reference types="x" />` (a type-only import of `x`) and `/// <reference path="./x.d.ts" />` (the file is
/// pulled in, so a side-effect import).
fn triple_slash_reference(ctx: &Ctx<'_>, text: &str, span: OxcSpan) -> Option<Import> {
    let tag = text.strip_prefix("///")?.trim();
    tag.strip_prefix("<reference")?;
    let span = ctx.span(span);
    if let Some(types) = attribute(tag, "types") {
        return Some(Import {
            specifier: types,
            kind: ImportKind::TypeOnly,
            names: vec![type_name(ImportedKind::Namespace, None, true)],
            span,
        });
    }
    let path = attribute(tag, "path")?;
    let specifier = if path.starts_with('.') || path.starts_with('/') {
        path
    } else {
        format!("./{path}")
    };
    Some(Import {
        specifier,
        kind: ImportKind::SideEffect,
        names: Vec::new(),
        span,
    })
}

fn type_name(kind: ImportedKind, local: Option<&str>, escapes: bool) -> ImportedName {
    ImportedName {
        kind,
        local: local.map(str::to_string),
        is_type: true,
        accessed_members: Vec::new(),
        escapes,
    }
}

/// The text of a block comment without its delimiters and line decorations, on one line.
fn flatten(comment: &str) -> String {
    comment
        .trim_start_matches("/**")
        .trim_end_matches("*/")
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$')
}

fn leading_ident(text: &str) -> &str {
    let end = text.find(|c: char| !is_ident_char(c)).unwrap_or(text.len());
    &text[..end]
}

fn quoted(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let quote = text.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let body = &text[1..];
    let end = body.find(quote)?;
    Some((&body[..end], &body[end + 1..]))
}

/// `@import {A, B as C} from 'pkg'`, `@import * as ns from 'pkg'` and `@import Def, {A} from 'pkg'`.
fn jsdoc_imports(flat: &str) -> Vec<(String, Vec<ImportedName>)> {
    let mut out = Vec::new();
    let mut rest = flat;
    while let Some(i) = rest.find("@import") {
        rest = &rest[i + "@import".len()..];
        if rest.starts_with(is_ident_char) {
            continue;
        }
        let Some((clause, after)) = rest.split_once(" from ") else {
            break;
        };
        let Some((specifier, _)) = quoted(after) else {
            continue;
        };
        if let Some(names) = import_clause(clause) {
            out.push((specifier.to_string(), names));
        }
    }
    out
}

fn import_clause(clause: &str) -> Option<Vec<ImportedName>> {
    let clause = clause.trim();
    if clause.is_empty() || !clause.chars().all(|c| is_ident_char(c) || " {},*".contains(c)) {
        return None;
    }
    let mut names = Vec::new();
    let head_end = clause.find(['{', '*']).unwrap_or(clause.len());
    let default = leading_ident(clause[..head_end].trim_start());
    if !default.is_empty() {
        names.push(type_name(ImportedKind::Default, Some(default), false));
    }
    if let Some(star) = clause.find('*') {
        let local = clause[star + 1..].trim_start().strip_prefix("as")?.trim_start();
        names.push(type_name(ImportedKind::Namespace, Some(leading_ident(local)), false));
    }
    if let (Some(open), Some(close)) = (clause.find('{'), clause.find('}'))
        && open < close
    {
        for item in clause[open + 1..close].split(',') {
            let mut words = item.split_whitespace().collect::<Vec<_>>();
            if words.first() == Some(&"type") && words.len() > 1 {
                words.remove(0);
            }
            let (imported, local) = match words.as_slice() {
                [name] => (*name, *name),
                [name, "as", local] => (*name, *local),
                _ => continue,
            };
            names.push(type_name(imported_kind(imported.to_string()), Some(local), false));
        }
    }
    (!names.is_empty()).then_some(names)
}

/// The `{...}` type expressions that directly follow a tag (`@type {…}`, `@param {…} name`), without the braces.
///
/// Braces elsewhere in the prose, including inline tags such as `{@link …}`, are not type expressions.
fn tag_type_expressions(flat: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = flat;
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let after_tag = rest[leading_ident(rest).len()..].trim_start();
        let Some(body) = after_tag.strip_prefix('{') else {
            continue;
        };
        let mut depth = 1usize;
        let end = body.char_indices().find_map(|(i, c)| {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        });
        let Some(end) = end else { break };
        out.push(&body[..end]);
        rest = &body[end + 1..];
    }
    out
}

/// `import('pkg')` and `import('pkg').Name` inside a `JSDoc` type expression.
fn jsdoc_import_types(flat: &str) -> Vec<(String, Vec<ImportedName>)> {
    const CALL: &str = "import(";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = flat[from..].find(CALL) {
        let at = from + i;
        from = at + CALL.len();
        if flat[..at]
            .chars()
            .next_back()
            .is_some_and(|c| is_ident_char(c) || c == '@')
        {
            continue;
        }
        let Some((specifier, after)) = quoted(&flat[from..]) else {
            continue;
        };
        let qualifier = after
            .trim_start()
            .strip_prefix(')')
            .and_then(|tail| tail.strip_prefix('.'))
            .map(leading_ident)
            .filter(|name| !name.is_empty());
        // A bare `import('m')` names the module's type without reading an export, as `typeof import('m')` does in
        // TypeScript, so it credits none.
        let name = match qualifier {
            Some(name) => type_name(imported_kind(name.to_string()), None, false),
            None => type_name(ImportedKind::Namespace, None, false),
        };
        out.push((specifier.to_string(), vec![name]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsdoc_import_clauses() {
        type Case<'a> = (&'a str, &'a str, &'a [(&'a str, Option<&'a str>)]);
        let cases: &[Case<'_>] = &[
            (
                "@import {A, B as C} from 'pkg'",
                "pkg",
                &[("A", Some("A")), ("B", Some("C"))],
            ),
            ("@import * as ns from \"pkg\"", "pkg", &[("*", Some("ns"))]),
            (
                "@import Def, {type A} from 'pkg'",
                "pkg",
                &[("default", Some("Def")), ("A", Some("A"))],
            ),
        ];
        for (src, specifier, expected) in cases {
            let got = jsdoc_imports(src);
            assert_eq!(got.len(), 1, "{src}");
            assert_eq!(got[0].0, *specifier, "{src}");
            let names: Vec<(String, Option<String>)> = got[0]
                .1
                .iter()
                .map(|n| {
                    let kind = match &n.kind {
                        ImportedKind::Default => "default".to_string(),
                        ImportedKind::Namespace => "*".to_string(),
                        ImportedKind::Named(n) => n.clone(),
                    };
                    (kind, n.local.clone())
                })
                .collect();
            let want: Vec<(String, Option<String>)> = expected
                .iter()
                .map(|(k, l)| ((*k).to_string(), l.map(str::to_string)))
                .collect();
            assert_eq!(names, want, "{src}");
        }
    }

    #[test]
    fn import_types_need_a_call_not_an_identifier_suffix() {
        assert_eq!(jsdoc_import_types("{import('a').B} and reimport('c')").len(), 1);
        assert_eq!(jsdoc_import_types("@import {X} from 'a'").len(), 0);
    }

    #[test]
    fn type_expressions_follow_tags_only() {
        assert_eq!(
            tag_type_expressions("@type {import('a').B} the {x: {y: 1}} @param {{k: import('c').D}} p"),
            ["import('a').B", "{k: import('c').D}"]
        );
        assert_eq!(
            tag_type_expressions("see {@link import('a')} or call import('b') here"),
            Vec::<&str>::new()
        );
        assert_eq!(tag_type_expressions("@returns {unterminated"), Vec::<&str>::new());
    }
}
