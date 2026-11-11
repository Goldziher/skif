//! Top-level import and export statements.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    ArrayExpressionElement, BindingPattern, Class, ClassElement, Declaration, ExportDefaultDeclarationKind,
    ExportSpecifier, Expression, ImportDeclarationSpecifier, ImportOrExportKind, ModuleExportName, ObjectPropertyKind,
    Program, Statement, StringLiteral, TSEnumMemberName, TSNamespaceDeclarationBody,
};
use oxc_span::Span as OxcSpan;
use skif_core::HashMap;
use skif_core::{LineIndex, Span};

use crate::jsdoc::leading_tags;
use crate::model::{
    Export, ExportKind, Import, ImportKind, ImportedKind, ImportedName, Member, ModuleInfo, Removal, RemovalKind,
};

/// Source block being analysed: its text, its offset in the original file and the file's line index.
pub(crate) struct Ctx<'s> {
    pub src: &'s str,
    pub base: u32,
    pub lines: &'s LineIndex,
}

impl Ctx<'_> {
    pub(crate) fn span(&self, s: OxcSpan) -> Span {
        self.lines.span(self.base + s.start, self.base + s.end)
    }

    pub(crate) fn off(&self, local: u32) -> u32 {
        self.base + local
    }

    fn tags_at(&self, start: u32) -> Vec<String> {
        leading_tags(self.src, start as usize)
    }
}

#[derive(Clone)]
struct Declared {
    name: String,
    name_span: OxcSpan,
    kind: ExportKind,
    is_type: bool,
    enum_members: Vec<Member>,
    namespace_members: Vec<Member>,
    /// `const b = a`: the identifier the declaration is initialised with.
    alias_of: Option<String>,
}

impl Declared {
    fn simple(name: &str, name_span: OxcSpan, kind: ExportKind, is_type: bool) -> Self {
        Self {
            name: name.to_string(),
            name_span,
            kind,
            is_type,
            enum_members: Vec::new(),
            namespace_members: Vec::new(),
            alias_of: None,
        }
    }
}

/// Result of the top-level pass.
pub(crate) struct Collected {
    /// Spans of identifiers inside export clauses; references there do not count as in-file use.
    pub export_clause_spans: Vec<OxcSpan>,
    /// `(start, end)` of the identifiers of `export { x }` clauses whose `x` is an imported binding. The clause is a
    /// re-export of the import, so it is not a use of the binding that escapes member tracking.
    pub reexport_spans: Vec<(u32, u32)>,
}

/// Spans collected from export clauses.
#[derive(Default)]
struct ClauseSpans {
    clause: Vec<OxcSpan>,
    reexport: Vec<(u32, u32)>,
}

/// The top-level names the export clauses can refer to.
struct Scope<'d> {
    declarations: &'d HashMap<String, Declared>,
    imports: HashMap<String, ImportBinding>,
}

/// What an imported local binding refers to, to recognise `import x from "m"; export { x }` as a re-export.
struct ImportBinding {
    specifier: String,
    kind: ImportedKind,
    is_type: bool,
}

/// Every binding the top-level import declarations introduce, by local name.
fn import_bindings(program: &Program<'_>) -> HashMap<String, ImportBinding> {
    let mut bindings = HashMap::default();
    for stmt in &program.body {
        let Statement::ImportDeclaration(d) = stmt else {
            continue;
        };
        let type_only = d.import_kind.is_type();
        let Some(specifiers) = &d.specifiers else { continue };
        for s in specifiers {
            let (kind, local, is_type) = match s {
                ImportDeclarationSpecifier::ImportSpecifier(s) => (
                    imported_kind(module_export_name(&s.imported)),
                    s.local.name.as_str(),
                    type_only || s.import_kind.is_type(),
                ),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (ImportedKind::Default, s.local.name.as_str(), type_only)
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    (ImportedKind::Namespace, s.local.name.as_str(), type_only)
                }
            };
            bindings.insert(
                local.to_string(),
                ImportBinding {
                    specifier: crate::js_string(d.source.value),
                    kind,
                    is_type,
                },
            );
        }
    }
    bindings
}

pub(crate) fn module_export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(i) => i.name.as_str().to_string(),
        ModuleExportName::IdentifierReference(i) => i.name.as_str().to_string(),
        ModuleExportName::StringLiteral(s) => crate::js_string(s.value),
    }
}

fn module_export_span(name: &ModuleExportName<'_>) -> OxcSpan {
    match name {
        ModuleExportName::IdentifierName(i) => i.span,
        ModuleExportName::IdentifierReference(i) => i.span,
        ModuleExportName::StringLiteral(s) => s.span,
    }
}

pub(crate) fn imported_kind(name: String) -> ImportedKind {
    if name == "default" {
        ImportedKind::Default
    } else {
        ImportedKind::Named(name)
    }
}

pub(crate) fn binding_ids(pattern: &BindingPattern<'_>, out: &mut Vec<(String, OxcSpan)>) {
    match pattern {
        BindingPattern::BindingIdentifier(b) => out.push((b.name.as_str().to_string(), b.span)),
        BindingPattern::ObjectPattern(o) => {
            for p in &o.properties {
                binding_ids(&p.value, out);
            }
            if let Some(rest) = &o.rest {
                binding_ids(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(a) => {
            for element in a.elements.iter().flatten() {
                binding_ids(element, out);
            }
            if let Some(rest) = &a.rest {
                binding_ids(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(a) => binding_ids(&a.left, out),
    }
}

fn enum_member_name(id: &TSEnumMemberName<'_>) -> Option<String> {
    match id {
        TSEnumMemberName::Identifier(i) => Some(i.name.as_str().to_string()),
        TSEnumMemberName::String(s) | TSEnumMemberName::ComputedString(s) => Some(crate::js_string(s.value)),
        TSEnumMemberName::ComputedTemplateString(t) if t.expressions.is_empty() => {
            t.quasis.first().map(|q| q.value.raw.as_str().to_string())
        }
        TSEnumMemberName::ComputedTemplateString(_) => None,
    }
}

fn declared(ctx: &Ctx<'_>, decl: &Declaration<'_>) -> Vec<Declared> {
    match decl {
        Declaration::VariableDeclaration(v) => {
            let mut out = Vec::new();
            for d in &v.declarations {
                let mut ids = Vec::new();
                binding_ids(&d.id, &mut ids);
                let alias_of = match (&d.id, d.init.as_ref()) {
                    (BindingPattern::BindingIdentifier(_), Some(oxc_ast::ast::Expression::Identifier(id))) => {
                        Some(id.name.as_str().to_string())
                    }
                    _ => None,
                };
                out.extend(ids.into_iter().map(|(name, span)| {
                    let mut declared = Declared::simple(&name, span, ExportKind::Value, false);
                    declared.alias_of.clone_from(&alias_of);
                    declared
                }));
            }
            out
        }
        Declaration::FunctionDeclaration(f) => {
            f.id.iter()
                .map(|id| Declared::simple(id.name.as_str(), id.span, ExportKind::Function, false))
                .collect()
        }
        Declaration::ClassDeclaration(c) => {
            c.id.iter()
                .map(|id| Declared::simple(id.name.as_str(), id.span, ExportKind::Class, false))
                .collect()
        }
        Declaration::TSTypeAliasDeclaration(t) => {
            vec![Declared::simple(t.id.name.as_str(), t.id.span, ExportKind::Type, true)]
        }
        Declaration::TSInterfaceDeclaration(t) => {
            vec![Declared::simple(t.id.name.as_str(), t.id.span, ExportKind::Type, true)]
        }
        Declaration::TSEnumDeclaration(e) => {
            let mut d = Declared::simple(e.id.name.as_str(), e.id.span, ExportKind::Enum, false);
            let members = &e.body.members;
            d.enum_members = members
                .iter()
                .enumerate()
                .filter_map(|(index, m)| {
                    enum_member_name(&m.id).map(|name| {
                        let mentioned = members.iter().any(|other| {
                            other.span != m.span
                                && mentions_word(&ctx.src[other.span.start as usize..other.span.end as usize], &name)
                        });
                        // A later member without an initializer takes its value from the one before it, so removing
                        // this member would renumber it.
                        let renumbers = members[index + 1..].iter().any(|later| later.initializer.is_none());
                        Member {
                            removal: if mentioned || renumbers {
                                None
                            } else {
                                member_removal(ctx, m.span, true)
                            },
                            name,
                            span: ctx.span(m.span),
                            tags: ctx.tags_at(m.span.start),
                        }
                    })
                })
                .collect();
            vec![d]
        }
        Declaration::TSNamespaceDeclaration(m) => {
            let mut d = Declared::simple(m.id.name.as_str(), m.id.span, ExportKind::Namespace, false);
            d.namespace_members = namespace_members(ctx, &m.body);
            vec![d]
        }
        #[allow(unreachable_patterns)]
        _ => Vec::new(),
    }
}

fn namespace_members(ctx: &Ctx<'_>, body: &TSNamespaceDeclarationBody<'_>) -> Vec<Member> {
    let mut out = Vec::new();
    match body {
        TSNamespaceDeclarationBody::TSModuleBlock(block) => {
            for stmt in &block.body {
                if let Statement::ExportDeclaration(e) = stmt {
                    let declared = declared(ctx, &e.declaration);
                    let alone = declared.len() == 1;
                    for d in declared {
                        // The rest of the namespace body, without this statement: a sibling that mentions the name
                        // would break when the statement goes.
                        let before = &ctx.src[block.span.start as usize..e.span.start as usize];
                        let after = &ctx.src[e.span.end as usize..block.span.end as usize];
                        let mentioned = mentions_word(before, &d.name) || mentions_word(after, &d.name);
                        out.push(Member {
                            removal: if alone && !mentioned {
                                member_removal(ctx, e.span, false)
                            } else {
                                None
                            },
                            name: d.name,
                            span: ctx.span(d.name_span),
                            tags: ctx.tags_at(e.span.start),
                        });
                    }
                }
            }
        }
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
            out.push(Member {
                name: inner.id.name.as_str().to_string(),
                span: ctx.span(inner.id.span),
                tags: Vec::new(),
                removal: None,
            });
        }
    }
    out
}

/// Whether `text` holds `name` as a whole identifier, not inside a longer one.
fn mentions_word(text: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    !name.is_empty()
        && text.match_indices(name).any(|(at, _)| {
            !text[..at].chars().next_back().is_some_and(is_ident)
                && !text[at + name.len()..].chars().next().is_some_and(is_ident)
        })
}

/// The range that deletes a member spanning `span` (absolute offsets). With `comma`, a comma right after it goes too.
///
/// A member alone on its line takes the whole line with its line break, so the remaining lines keep their layout,
/// together with a comment after it on that line and the comment lines directly above it, which describe it. A member
/// sharing its line loses only itself, its comma and the blanks after it; `None` when a comment sits right before or
/// after it, which would be left describing a neighbour.
fn member_removal(ctx: &Ctx<'_>, span: OxcSpan, comma: bool) -> Option<(u32, u32)> {
    let src = ctx.src;
    let bytes = src.as_bytes();
    let blank = |b: u8| b == b' ' || b == b'\t';
    let mut end = span.end as usize;
    if comma {
        let mut i = end;
        while i < bytes.len() && blank(bytes[i]) {
            i += 1;
        }
        if bytes.get(i) == Some(&b',') {
            end = i + 1;
        }
    }
    let mut after = end;
    while after < bytes.len() && blank(bytes[after]) {
        after += 1;
    }
    let mut line_start = span.start as usize;
    while line_start > 0 && blank(bytes[line_start - 1]) {
        line_start -= 1;
    }
    let starts_line = line_start == 0 || bytes[line_start - 1] == b'\n';
    let line_end = |at: usize| {
        if bytes[at..].starts_with(b"\r\n") {
            Some(at + 2)
        } else if bytes.get(at) == Some(&b'\n') {
            Some(at + 1)
        } else {
            None
        }
    };
    let (start, end) = match line_end(skip_line_comment(src, after)) {
        Some(line_end) if starts_line => (comment_lines_above(src, line_start), line_end),
        _ if src[..span.start as usize].trim_end().ends_with("*/")
            || (starts_line && comment_lines_above(src, line_start) < line_start)
            || trailing_comment(src, after) =>
        {
            return None;
        }
        _ => (span.start as usize, after),
    };
    Some((ctx.off(u32::try_from(start).ok()?), ctx.off(u32::try_from(end).ok()?)))
}

/// Whether the comment at `at` belongs to the member before it: a `//` comment, or a block comment that ends the
/// line or the body rather than leading into the next member.
fn trailing_comment(src: &str, at: usize) -> bool {
    let rest = &src[at..];
    if rest.starts_with("//") {
        return true;
    }
    rest.strip_prefix("/*")
        .and_then(|body| {
            body.find("*/")
                .map(|close| body[close + 2..].trim_start_matches([' ', '\t']))
        })
        .is_some_and(|next| next.is_empty() || next.starts_with(['\r', '\n', '}']))
}

/// `at` moved past a `//` comment, or a block comment closed on the same line, and the blanks after it.
fn skip_line_comment(src: &str, at: usize) -> usize {
    let rest = &src[at..];
    let line = rest.find('\n').map_or(rest, |newline| &rest[..newline]);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.starts_with("//") {
        return at + line.len();
    }
    if line.starts_with("/*")
        && let Some(close) = line.find("*/")
        && line[close + 2..].trim().is_empty()
    {
        return at + line.len();
    }
    at
}

/// The start of the run of comment lines (`//` lines and block comments that open at the start of a line) directly
/// above the line starting at `line_start`, or `line_start` when there is none. A blank line ends the run.
fn comment_lines_above(src: &str, line_start: usize) -> usize {
    let mut start = line_start;
    while start > 0 {
        let before = &src[..start - 1];
        let previous_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        let previous = before[previous_start..].trim();
        if previous.starts_with("//") {
            start = previous_start;
            continue;
        }
        if !previous.ends_with("*/") {
            break;
        }
        let Some(open) = before.rfind("/*") else { break };
        let open_line = before[..open].rfind('\n').map_or(0, |newline| newline + 1);
        if !before[open_line..open].trim().is_empty() {
            break;
        }
        start = open_line;
    }
    start
}

/// Range of a statement extended over one trailing line break (absolute offsets).
fn statement_range(ctx: &Ctx<'_>, span: OxcSpan) -> (u32, u32) {
    let rest = &ctx.src.as_bytes()[span.end as usize..];
    let mut i = 0;
    while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
        i += 1;
    }
    let extra = if rest[i..].starts_with(b"\r\n") {
        i + 2
    } else if rest[i..].starts_with(b"\n") {
        i + 1
    } else {
        0
    };
    (
        ctx.off(span.start),
        ctx.off(span.end) + u32::try_from(extra).unwrap_or(0),
    )
}

/// Skips whitespace and comments from `at`.
fn skip_trivia(src: &str, mut at: usize) -> usize {
    loop {
        let rest = &src[at..];
        let trimmed = rest.trim_start();
        at += rest.len() - trimmed.len();
        if trimmed.starts_with("//") {
            at += trimmed.find('\n').unwrap_or(trimmed.len());
        } else if let Some(body) = trimmed.strip_prefix("/*") {
            at += body.find("*/").map_or(trimmed.len(), |i| i + 4);
        } else {
            return at;
        }
    }
}

/// Skips one decorator (`@a.b(...)`) starting at `at`, which must hold the `@`.
fn skip_decorator(src: &str, at: usize) -> usize {
    let bytes = src.as_bytes();
    let mut i = at + 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'$' | b'.')) {
        i += 1;
    }
    let j = skip_trivia(src, i);
    if bytes.get(j) != Some(&b'(') {
        return i;
    }
    let (mut depth, mut quote) = (0_u32, None);
    for (k, &b) in bytes.iter().enumerate().skip(j) {
        match (quote, b) {
            (Some(q), b) if b == q => quote = None,
            (None, b'"' | b'\'' | b'`') => quote = Some(b),
            (None, b'(') => depth += 1,
            (None, b')') => {
                depth -= 1;
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
    }
    bytes.len()
}

fn keyword_at(src: &str, at: usize, keyword: &str) -> bool {
    src[at..].starts_with(keyword)
        && !src[at + keyword.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
}

/// Range of the `export` (or `export default`) keyword and the whitespace after it (absolute offsets).
///
/// The keyword is located lexically from the start of the statement, past any leading decorators and comments, so
/// text such as `@Component({ selector: 'export' })` is never mistaken for it.
fn keyword_range(ctx: &Ctx<'_>, stmt: OxcSpan, default: bool) -> Option<(u32, u32)> {
    let src = ctx.src;
    let mut kw = skip_trivia(src, stmt.start as usize);
    while src[kw..].starts_with('@') {
        kw = skip_trivia(src, skip_decorator(src, kw));
    }
    keyword_at(src, kw, "export").then_some(())?;
    let mut end = kw + "export".len();
    if default {
        let at = skip_trivia(src, end);
        keyword_at(src, at, "default").then_some(())?;
        end = at + "default".len();
    }
    end = skip_trivia(src, end);
    Some((ctx.off(u32::try_from(kw).ok()?), ctx.off(u32::try_from(end).ok()?)))
}

fn removal(ctx: &Ctx<'_>, stmt: OxcSpan, range: (u32, u32), kind: RemovalKind, siblings: usize) -> Removal {
    Removal {
        start: range.0,
        end: range.1,
        kind,
        statement: statement_range(ctx, stmt),
        siblings: u32::try_from(siblings).unwrap_or(u32::MAX),
        default_expression: false,
    }
}

fn whole_statement(ctx: &Ctx<'_>, stmt: OxcSpan, siblings: usize) -> Removal {
    let range = statement_range(ctx, stmt);
    removal(ctx, stmt, range, RemovalKind::Statement, siblings)
}

fn export_from_declared(ctx: &Ctx<'_>, d: Declared, tags: Vec<String>, removal: Option<Removal>) -> Export {
    Export {
        local_name: Some(d.name.clone()),
        name: d.name,
        kind: d.kind,
        is_type: d.is_type,
        from: None,
        imported_name: None,
        tags,
        span: ctx.span(d.name_span),
        enum_members: d.enum_members,
        namespace_members: d.namespace_members,
        removal,
        referenced_in_file: false,
        referenced_in: Vec::new(),
        members_read_in_file: Vec::new(),
        alias_of: d.alias_of,
        registrations: Vec::new(),
    }
}

impl Export {
    pub(crate) fn bare(name: String, kind: ExportKind, span: Span) -> Self {
        Self {
            name,
            kind,
            is_type: false,
            local_name: None,
            from: None,
            imported_name: None,
            tags: Vec::new(),
            span,
            enum_members: Vec::new(),
            namespace_members: Vec::new(),
            removal: None,
            referenced_in_file: false,
            referenced_in: Vec::new(),
            members_read_in_file: Vec::new(),
            alias_of: None,
            registrations: Vec::new(),
        }
    }
}

/// Collects imports and exports from the top-level statements of `program`.
pub(crate) fn collect(ctx: &Ctx<'_>, program: &Program<'_>, info: &mut ModuleInfo) -> Collected {
    let mut local_decls: HashMap<String, Declared> = HashMap::default();
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(decl) = decl {
            for d in declared(ctx, decl) {
                local_decls.insert(d.name.clone(), d);
            }
        }
    }

    let scope = Scope {
        declarations: &local_decls,
        imports: import_bindings(program),
    };
    let mut spans = ClauseSpans::default();
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(d) => collect_import(ctx, d, info),
            Statement::ExportDeclaration(e) => {
                let named = NamedExport {
                    span: e.span,
                    declaration: Some(&e.declaration),
                    specifiers: &[],
                    source: None,
                    export_kind: ImportOrExportKind::Value,
                };
                collect_named_export(ctx, &named, &scope, info, &mut spans);
            }
            Statement::ExportNamedDeclaration(e) => {
                let named = NamedExport {
                    span: e.span,
                    declaration: None,
                    specifiers: &e.specifiers,
                    source: None,
                    export_kind: e.export_kind,
                };
                collect_named_export(ctx, &named, &scope, info, &mut spans);
            }
            Statement::ExportFromDeclaration(e) => {
                let named = NamedExport {
                    span: e.span,
                    declaration: None,
                    specifiers: &e.specifiers,
                    source: Some(&e.source),
                    export_kind: e.export_kind,
                };
                collect_named_export(ctx, &named, &scope, info, &mut spans);
            }
            Statement::ExportDefaultDeclaration(e) => {
                collect_default_export(ctx, e, info, &mut spans.clause);
            }
            Statement::ExportAllDeclaration(e) => {
                let span = ctx.span(e.span);
                let from = crate::js_string(e.source.value);
                let name = e.exported.as_ref().map_or_else(|| "*".to_string(), module_export_name);
                let is_type = e.export_kind.is_type();
                let mut export = Export::bare(name, ExportKind::StarReExport, span);
                export.is_type = is_type;
                export.from = Some(from.clone());
                export.imported_name = Some("*".to_string());
                export.tags = ctx.tags_at(e.span.start);
                export.removal = Some(whole_statement(ctx, e.span, 1));
                info.exports.push(export);
                info.imports.push(Import {
                    specifier: from,
                    kind: ImportKind::ExportFrom,
                    names: vec![ImportedName {
                        kind: ImportedKind::Namespace,
                        local: None,
                        is_type,
                        accessed_members: Vec::new(),
                        nested_members: BTreeMap::new(),
                        escapes: false,
                    }],
                    call: None,
                    span,
                });
            }
            Statement::TSExportAssignment(e) => {
                let mut export = Export::bare("export=".to_string(), ExportKind::Value, ctx.span(e.span));
                export.tags = ctx.tags_at(e.span.start);
                if let oxc_ast::ast::Expression::Identifier(id) = &e.expression {
                    spans.clause.push(id.span);
                    export.local_name = Some(id.name.as_str().to_string());
                }
                info.exports.push(export);
            }
            _ => {}
        }
    }
    Collected {
        export_clause_spans: spans.clause,
        reexport_spans: spans.reexport,
    }
}

pub(crate) fn collect_import(ctx: &Ctx<'_>, d: &oxc_ast::ast::ImportDeclaration<'_>, info: &mut ModuleInfo) {
    let type_only = d.import_kind.is_type();
    let mut names = Vec::new();
    if let Some(specifiers) = &d.specifiers {
        for s in specifiers {
            let (kind, local, is_type) = match s {
                ImportDeclarationSpecifier::ImportSpecifier(s) => (
                    imported_kind(module_export_name(&s.imported)),
                    s.local.name.as_str(),
                    type_only || s.import_kind.is_type(),
                ),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (ImportedKind::Default, s.local.name.as_str(), type_only)
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    (ImportedKind::Namespace, s.local.name.as_str(), type_only)
                }
            };
            names.push(ImportedName {
                kind,
                local: Some(local.to_string()),
                is_type,
                accessed_members: Vec::new(),
                nested_members: BTreeMap::new(),
                escapes: false,
            });
        }
    }
    let kind = if names.is_empty() {
        ImportKind::SideEffect
    } else if type_only {
        ImportKind::TypeOnly
    } else {
        ImportKind::Static
    };
    info.imports.push(Import {
        specifier: crate::js_string(d.source.value),
        kind,
        names,
        call: None,
        span: ctx.span(d.span),
    });
}

/// The parts shared by `export <declaration>`, `export { a }` and `export { a } from "m"`, which oxc
/// models as three node types.
struct NamedExport<'n, 'a> {
    span: OxcSpan,
    declaration: Option<&'n Declaration<'a>>,
    specifiers: &'n [ExportSpecifier<'a>],
    source: Option<&'n StringLiteral<'a>>,
    export_kind: ImportOrExportKind,
}

fn collect_named_export(
    ctx: &Ctx<'_>,
    e: &NamedExport<'_, '_>,
    scope: &Scope<'_>,
    info: &mut ModuleInfo,
    spans: &mut ClauseSpans,
) {
    let stmt_tags = ctx.tags_at(e.span.start);
    if let Some(decl) = e.declaration {
        let items = declared(ctx, decl);
        let siblings = items.len();
        let kind = if siblings > 1 {
            RemovalKind::SharedExportKeyword
        } else {
            RemovalKind::ExportKeyword
        };
        let rem = keyword_range(ctx, e.span, false).map(|range| removal(ctx, e.span, range, kind, siblings));
        for d in items {
            info.exports.push(export_from_declared(ctx, d, stmt_tags.clone(), rem));
        }
        return;
    }

    let count = e.specifiers.len();
    let mut from_names = Vec::new();
    for (i, s) in e.specifiers.iter().enumerate() {
        let local = module_export_name(&s.local);
        let exported = module_export_name(&s.exported);
        let mut tags = leading_tags(ctx.src, s.span.start as usize);
        if i == 0 {
            for t in &stmt_tags {
                if !tags.contains(t) {
                    tags.push(t.clone());
                }
            }
        }
        let is_type = e.export_kind.is_type() || s.export_kind.is_type();
        let range = if count == 1 {
            statement_range(ctx, e.span)
        } else if i + 1 < count {
            (ctx.off(s.span.start), ctx.off(e.specifiers[i + 1].span.start))
        } else {
            (ctx.off(e.specifiers[i - 1].span.end), ctx.off(s.span.end))
        };
        let kind = if count == 1 {
            RemovalKind::Statement
        } else {
            RemovalKind::Specifier
        };
        let rem = removal(ctx, e.span, range, kind, count);

        if let Some(source) = e.source {
            let mut export = Export::bare(exported, ExportKind::ReExport, ctx.span(s.span));
            export.is_type = is_type;
            export.from = Some(crate::js_string(source.value));
            export.imported_name = Some(local.clone());
            export.tags = tags;
            export.removal = Some(rem);
            info.exports.push(export);
            from_names.push(ImportedName {
                kind: imported_kind(local),
                local: None,
                is_type,
                accessed_members: Vec::new(),
                nested_members: BTreeMap::new(),
                escapes: false,
            });
        } else if let Some(binding) = scope.imports.get(&local) {
            let name_span = module_export_span(&s.local);
            spans.clause.push(name_span);
            spans.reexport.push((name_span.start, name_span.end));
            let (kind, imported) = match &binding.kind {
                ImportedKind::Namespace => (ExportKind::StarReExport, "*".to_owned()),
                ImportedKind::Default => (ExportKind::ReExport, "default".to_owned()),
                ImportedKind::Named(name) => (ExportKind::ReExport, name.clone()),
            };
            let mut export = Export::bare(exported, kind, ctx.span(s.span));
            export.is_type = is_type || binding.is_type;
            export.local_name = Some(local);
            export.from = Some(binding.specifier.clone());
            export.imported_name = Some(imported);
            export.tags = tags;
            export.removal = Some(rem);
            info.exports.push(export);
        } else {
            spans.clause.push(module_export_span(&s.local));
            let decl = scope.declarations.get(&local);
            let mut export = Export::bare(
                exported.clone(),
                decl.map_or(ExportKind::Value, |d| d.kind),
                ctx.span(s.span),
            );
            export.is_type = is_type || decl.is_some_and(|d| d.is_type);
            if exported == "default" {
                export.kind = ExportKind::Default;
            }
            if let Some(d) = decl {
                export.enum_members.clone_from(&d.enum_members);
                export.namespace_members.clone_from(&d.namespace_members);
            }
            export.local_name = Some(local);
            export.tags = tags;
            export.removal = Some(rem);
            info.exports.push(export);
        }
    }
    if let Some(source) = e.source {
        info.imports.push(Import {
            specifier: crate::js_string(source.value),
            kind: ImportKind::ExportFrom,
            names: from_names,
            call: None,
            span: ctx.span(e.span),
        });
    }
}

fn collect_default_export(
    ctx: &Ctx<'_>,
    e: &oxc_ast::ast::ExportDefaultDeclaration<'_>,
    info: &mut ModuleInfo,
    clause_spans: &mut Vec<OxcSpan>,
) {
    let mut export = Export::bare("default".to_string(), ExportKind::Default, ctx.span(e.span));
    export.tags = ctx.tags_at(e.span.start);
    let keep_declaration = match &e.declaration {
        ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
            export.local_name = f.id.as_ref().map(|id| id.name.as_str().to_string());
            f.id.is_some()
        }
        ExportDefaultDeclarationKind::ClassDeclaration(c) => {
            export.local_name = c.id.as_ref().map(|id| id.name.as_str().to_string());
            c.id.is_some()
        }
        ExportDefaultDeclarationKind::TSInterfaceDeclaration(i) => {
            export.local_name = Some(i.id.name.as_str().to_string());
            export.is_type = true;
            true
        }
        other => {
            if let Some(oxc_ast::ast::Expression::Identifier(id)) = other.as_expression() {
                export.local_name = Some(id.name.as_str().to_string());
                clause_spans.push(id.span);
            }
            false
        }
    };
    let pure = match &e.declaration {
        ExportDefaultDeclarationKind::FunctionDeclaration(_)
        | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => true,
        ExportDefaultDeclarationKind::ClassDeclaration(c) => pure_class(c),
        other => other.as_expression().is_some_and(pure_expression),
    };
    export.removal = if keep_declaration {
        keyword_range(ctx, e.span, true).map(|range| removal(ctx, e.span, range, RemovalKind::ExportKeyword, 1))
    } else if pure {
        let mut whole = whole_statement(ctx, e.span, 1);
        whole.default_expression = true;
        Some(whole)
    } else {
        None
    };
    info.exports.push(export);
}

/// Whether evaluating `expression` can have no effect beyond producing its value: literals, identifiers, functions,
/// classes without evaluated members, and arrays and objects of such values. Deleting `export default <expression>`
/// is only offered for these, so a call such as `export default registerPlugin()` keeps running.
fn pure_expression(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::Identifier(_)
        | Expression::FunctionExpression(_)
        | Expression::ArrowFunctionExpression(_) => true,
        Expression::TemplateLiteral(t) => t.expressions.is_empty(),
        Expression::UnaryExpression(u) => {
            matches!(u.argument, Expression::NumericLiteral(_) | Expression::BigIntLiteral(_))
        }
        Expression::ClassExpression(c) => pure_class(c),
        Expression::ArrayExpression(a) => a.elements.iter().all(|element| match element {
            ArrayExpressionElement::SpreadElement(_) => false,
            ArrayExpressionElement::Elision(_) => true,
            element => element.as_expression().is_some_and(pure_expression),
        }),
        Expression::ObjectExpression(o) => o.properties.iter().all(|property| match property {
            ObjectPropertyKind::ObjectProperty(p) => !p.computed && pure_expression(&p.value),
            ObjectPropertyKind::SpreadProperty(_) => false,
        }),
        Expression::ParenthesizedExpression(p) => pure_expression(&p.expression),
        Expression::TSAsExpression(t) => pure_expression(&t.expression),
        Expression::TSSatisfiesExpression(t) => pure_expression(&t.expression),
        Expression::TSNonNullExpression(t) => pure_expression(&t.expression),
        Expression::TSTypeAssertion(t) => pure_expression(&t.expression),
        _ => false,
    }
}

/// Whether defining `class` runs no code of the module: no decorators, a plain superclass, no computed keys, no static
/// blocks and no static field initialisers with effects.
fn pure_class(class: &Class<'_>) -> bool {
    class.decorators.is_empty()
        && class
            .heritage
            .as_ref()
            .is_none_or(|heritage| matches!(heritage.expression, Expression::Identifier(_)))
        && class.body.body.iter().all(|element| match element {
            ClassElement::StaticBlock(_) => false,
            ClassElement::MethodDefinition(m) => m.decorators.is_empty() && !m.computed,
            ClassElement::PropertyDefinition(p) => {
                p.decorators.is_empty() && !p.computed && (!p.r#static || p.value.as_ref().is_none_or(pure_expression))
            }
            ClassElement::AccessorProperty(p) => {
                p.decorators.is_empty() && !p.computed && (!p.r#static || p.value.as_ref().is_none_or(pure_expression))
            }
            ClassElement::TSIndexSignature(_) => true,
        })
}
