//! Top-level import and export statements.

use std::collections::HashMap;

use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, ImportDeclarationSpecifier, ModuleExportName, Program,
    Statement, TSEnumMemberName, TSModuleDeclarationBody, TSModuleDeclarationName,
};
use oxc_span::Span as OxcSpan;
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
        }
    }
}

/// Result of the top-level pass.
pub(crate) struct Collected {
    /// Spans of identifiers inside export clauses; references there do not count as in-file use.
    pub export_clause_spans: Vec<OxcSpan>,
}

pub(crate) fn module_export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(i) => i.name.as_str().to_string(),
        ModuleExportName::IdentifierReference(i) => i.name.as_str().to_string(),
        ModuleExportName::StringLiteral(s) => s.value.as_str().to_string(),
    }
}

fn module_export_span(name: &ModuleExportName<'_>) -> OxcSpan {
    match name {
        ModuleExportName::IdentifierName(i) => i.span,
        ModuleExportName::IdentifierReference(i) => i.span,
        ModuleExportName::StringLiteral(s) => s.span,
    }
}

fn imported_kind(name: String) -> ImportedKind {
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
        TSEnumMemberName::String(s) | TSEnumMemberName::ComputedString(s) => Some(s.value.as_str().to_string()),
        TSEnumMemberName::ComputedTemplateString(t) if t.expressions.is_empty() => {
            t.quasis.first().map(|q| q.value.raw.as_str().to_string())
        }
        TSEnumMemberName::ComputedTemplateString(_) => None,
    }
}

fn declared(ctx: &Ctx<'_>, decl: &Declaration<'_>) -> Vec<Declared> {
    match decl {
        Declaration::VariableDeclaration(v) => {
            let mut ids = Vec::new();
            for d in &v.declarations {
                binding_ids(&d.id, &mut ids);
            }
            ids.into_iter()
                .map(|(name, span)| Declared::simple(&name, span, ExportKind::Value, false))
                .collect()
        }
        Declaration::FunctionDeclaration(f) => f
            .id
            .iter()
            .map(|id| Declared::simple(id.name.as_str(), id.span, ExportKind::Function, false))
            .collect(),
        Declaration::ClassDeclaration(c) => c
            .id
            .iter()
            .map(|id| Declared::simple(id.name.as_str(), id.span, ExportKind::Class, false))
            .collect(),
        Declaration::TSTypeAliasDeclaration(t) => {
            vec![Declared::simple(t.id.name.as_str(), t.id.span, ExportKind::Type, true)]
        }
        Declaration::TSInterfaceDeclaration(t) => {
            vec![Declared::simple(t.id.name.as_str(), t.id.span, ExportKind::Type, true)]
        }
        Declaration::TSEnumDeclaration(e) => {
            let mut d = Declared::simple(e.id.name.as_str(), e.id.span, ExportKind::Enum, false);
            d.enum_members = e
                .body
                .members
                .iter()
                .filter_map(|m| {
                    enum_member_name(&m.id).map(|name| Member {
                        name,
                        span: ctx.span(m.span),
                        tags: ctx.tags_at(m.span.start),
                    })
                })
                .collect();
            vec![d]
        }
        Declaration::TSModuleDeclaration(m) => match &m.id {
            TSModuleDeclarationName::Identifier(id) => {
                let mut d = Declared::simple(id.name.as_str(), id.span, ExportKind::Namespace, false);
                d.namespace_members = namespace_members(ctx, m.body.as_ref());
                vec![d]
            }
            TSModuleDeclarationName::StringLiteral(_) => Vec::new(),
        },
        #[allow(unreachable_patterns)]
        _ => Vec::new(),
    }
}

fn namespace_members(ctx: &Ctx<'_>, body: Option<&TSModuleDeclarationBody<'_>>) -> Vec<Member> {
    let mut out = Vec::new();
    match body {
        Some(TSModuleDeclarationBody::TSModuleBlock(block)) => {
            for stmt in &block.body {
                if let Statement::ExportNamedDeclaration(e) = stmt
                    && let Some(decl) = &e.declaration
                {
                    for d in declared(ctx, decl) {
                        out.push(Member {
                            name: d.name,
                            span: ctx.span(d.name_span),
                            tags: ctx.tags_at(e.span.start),
                        });
                    }
                }
            }
        }
        Some(TSModuleDeclarationBody::TSModuleDeclaration(inner)) => {
            if let TSModuleDeclarationName::Identifier(id) = &inner.id {
                out.push(Member {
                    name: id.name.as_str().to_string(),
                    span: ctx.span(id.span),
                    tags: Vec::new(),
                });
            }
        }
        None => {}
    }
    out
}

/// Range of a statement extended over one trailing line break (absolute offsets).
fn statement_range(ctx: &Ctx<'_>, span: OxcSpan) -> (u32, u32) {
    let rest = ctx.src[span.end as usize..].as_bytes();
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
    (ctx.off(span.start), ctx.off(span.end) + u32::try_from(extra).unwrap_or(0))
}

/// Range of the `export` (or `export default`) keyword and the whitespace after it (absolute offsets).
fn keyword_range(ctx: &Ctx<'_>, stmt: OxcSpan, default: bool) -> Option<(u32, u32)> {
    let from = stmt.start as usize;
    let kw = from + ctx.src[from..].find("export")?;
    let mut end = kw + "export".len();
    if default {
        let tail = ctx.src[end..].trim_start();
        let skipped = ctx.src.len() - end - tail.len();
        tail.starts_with("default").then_some(())?;
        end += skipped + "default".len();
    }
    let tail = &ctx.src[end..];
    end += tail.len() - tail.trim_start().len();
    Some((ctx.off(u32::try_from(kw).ok()?), ctx.off(u32::try_from(end).ok()?)))
}

fn removal(ctx: &Ctx<'_>, stmt: OxcSpan, range: (u32, u32), kind: RemovalKind, siblings: usize) -> Removal {
    Removal {
        start: range.0,
        end: range.1,
        kind,
        statement: statement_range(ctx, stmt),
        siblings: u32::try_from(siblings).unwrap_or(u32::MAX),
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
    }
}

impl Export {
    fn bare(name: String, kind: ExportKind, span: Span) -> Self {
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
        }
    }
}

/// Collects imports and exports from the top-level statements of `program`.
pub(crate) fn collect(ctx: &Ctx<'_>, program: &Program<'_>, info: &mut ModuleInfo) -> Collected {
    let mut local_decls: HashMap<String, Declared> = HashMap::new();
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportNamedDeclaration(e) => e.declaration.as_ref(),
            other => other.as_declaration(),
        };
        if let Some(decl) = decl {
            for d in declared(ctx, decl) {
                local_decls.insert(d.name.clone(), d);
            }
        }
    }

    let mut export_clause_spans = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(d) => collect_import(ctx, d, info),
            Statement::ExportNamedDeclaration(e) => {
                collect_named_export(ctx, e, &local_decls, info, &mut export_clause_spans);
            }
            Statement::ExportDefaultDeclaration(e) => {
                collect_default_export(ctx, e, info, &mut export_clause_spans);
            }
            Statement::ExportAllDeclaration(e) => {
                let span = ctx.span(e.span);
                let from = e.source.value.as_str().to_string();
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
                        escapes: false,
                    }],
                    span,
                });
            }
            Statement::TSExportAssignment(e) => {
                let mut export = Export::bare("export=".to_string(), ExportKind::Value, ctx.span(e.span));
                export.tags = ctx.tags_at(e.span.start);
                if let oxc_ast::ast::Expression::Identifier(id) = &e.expression {
                    export_clause_spans.push(id.span);
                    export.local_name = Some(id.name.as_str().to_string());
                }
                info.exports.push(export);
            }
            _ => {}
        }
    }
    Collected { export_clause_spans }
}

fn collect_import(ctx: &Ctx<'_>, d: &oxc_ast::ast::ImportDeclaration<'_>, info: &mut ModuleInfo) {
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
        specifier: d.source.value.as_str().to_string(),
        kind,
        names,
        span: ctx.span(d.span),
    });
}

fn collect_named_export(
    ctx: &Ctx<'_>,
    e: &oxc_ast::ast::ExportNamedDeclaration<'_>,
    local_decls: &HashMap<String, Declared>,
    info: &mut ModuleInfo,
    clause_spans: &mut Vec<OxcSpan>,
) {
    let stmt_tags = ctx.tags_at(e.span.start);
    if let Some(decl) = &e.declaration {
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
        let kind = if count == 1 { RemovalKind::Statement } else { RemovalKind::Specifier };
        let rem = removal(ctx, e.span, range, kind, count);

        if let Some(source) = &e.source {
            let mut export = Export::bare(exported, ExportKind::ReExport, ctx.span(s.span));
            export.is_type = is_type;
            export.from = Some(source.value.as_str().to_string());
            export.imported_name = Some(local.clone());
            export.tags = tags;
            export.removal = Some(rem);
            info.exports.push(export);
            from_names.push(ImportedName {
                kind: imported_kind(local),
                local: None,
                is_type,
                accessed_members: Vec::new(),
                escapes: false,
            });
        } else {
            clause_spans.push(module_export_span(&s.local));
            let decl = local_decls.get(&local);
            let mut export = Export::bare(exported.clone(), decl.map_or(ExportKind::Value, |d| d.kind), ctx.span(s.span));
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
    if let Some(source) = &e.source {
        info.imports.push(Import {
            specifier: source.value.as_str().to_string(),
            kind: ImportKind::ExportFrom,
            names: from_names,
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
    export.removal = if keep_declaration {
        keyword_range(ctx, e.span, true).map(|range| removal(ctx, e.span, range, RemovalKind::ExportKeyword, 1))
    } else {
        Some(whole_statement(ctx, e.span, 1))
    };
    info.exports.push(export);
}
