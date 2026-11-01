//! Nested module references: `require`, dynamic `import()`, `import.meta.glob`, `import x = require()`, `CommonJS`
//! exports and JSX.

use std::collections::HashMap;

use oxc_ast::ast::{
    Argument, AssignmentExpression, AssignmentTarget, BindingPattern, CallExpression, Expression, ImportExpression,
    JSXElement, JSXFragment, StaticMemberExpression, TSImportEqualsDeclaration, TSModuleReference, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};

use crate::collect::Ctx;
use crate::model::{Import, ImportKind, ImportedKind, ImportedName, ModuleInfo};

pub(crate) struct Finder<'c, 's> {
    ctx: &'c Ctx<'s>,
    info: &'c mut ModuleInfo,
    /// Bindings for a call or `import()` expression, keyed by the expression's start offset.
    pending: HashMap<u32, Vec<ImportedName>>,
}

impl<'c, 's> Finder<'c, 's> {
    pub(crate) fn new(ctx: &'c Ctx<'s>, info: &'c mut ModuleInfo) -> Self {
        Self {
            ctx,
            info,
            pending: HashMap::new(),
        }
    }
}

fn name(kind: ImportedKind, local: Option<&str>, escapes: bool) -> ImportedName {
    ImportedName {
        kind,
        local: local.map(str::to_string),
        is_type: false,
        accessed_members: Vec::new(),
        escapes,
    }
}

fn escaping_namespace() -> Vec<ImportedName> {
    vec![name(ImportedKind::Namespace, None, true)]
}

/// Constant string value of an expression: a string literal or a template without expressions.
pub(crate) fn static_string(expr: &Expression<'_>) -> Option<String> {
    match expr.get_inner_expression() {
        Expression::StringLiteral(s) => Some(crate::js_string(s.value)),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => t.quasis.first().map(|q| {
            q.value
                .cooked
                .map_or_else(|| q.value.raw.as_str().to_owned(), crate::js_string)
        }),
        _ => None,
    }
}

fn is_ident(expr: &Expression<'_>, wanted: &str) -> bool {
    matches!(expr, Expression::Identifier(id) if id.name.as_str() == wanted)
}

fn is_import_meta(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::ImportMeta(_))
}

fn is_module_exports(member: &StaticMemberExpression<'_>) -> bool {
    is_ident(&member.object, "module") && member.property.name.as_str() == "exports"
}

fn is_cjs_member(member: &StaticMemberExpression<'_>) -> bool {
    if is_module_exports(member) {
        return true;
    }
    match &member.object {
        Expression::Identifier(id) => id.name.as_str() == "exports",
        Expression::StaticMemberExpression(inner) => is_module_exports(inner),
        _ => false,
    }
}

/// Is `expr` (after unwrapping `await` and parentheses) a `require("…")` call or `import("…")`? Returns its start.
fn module_call_start(expr: &Expression<'_>) -> Option<u32> {
    let inner = match expr.get_inner_expression() {
        Expression::AwaitExpression(a) => a.argument.get_inner_expression(),
        other => other,
    };
    match inner {
        Expression::ImportExpression(i) => Some(i.span.start),
        Expression::CallExpression(c) if is_ident(&c.callee, "require") => Some(c.span.start),
        _ => None,
    }
}

fn bindings_for_pattern(pattern: &BindingPattern<'_>) -> Vec<ImportedName> {
    match pattern {
        BindingPattern::BindingIdentifier(b) => vec![name(ImportedKind::Namespace, Some(b.name.as_str()), false)],
        BindingPattern::ObjectPattern(o) => {
            let mut out = Vec::new();
            for p in &o.properties {
                let Some(key) = p.key.static_name() else {
                    out.push(name(ImportedKind::Namespace, None, true));
                    continue;
                };
                let kind = if key == "default" {
                    ImportedKind::Default
                } else {
                    ImportedKind::Named(crate::js_string(key.as_js_str()))
                };
                let target = match &p.value {
                    BindingPattern::AssignmentPattern(a) => &a.left,
                    other => other,
                };
                match target {
                    BindingPattern::BindingIdentifier(b) => out.push(name(kind, Some(b.name.as_str()), false)),
                    _ => out.push(name(kind, None, true)),
                }
            }
            if o.rest.is_some() {
                out.push(name(ImportedKind::Namespace, None, true));
            }
            out
        }
        _ => escaping_namespace(),
    }
}

impl<'a> Visit<'a> for Finder<'_, '_> {
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init
            && let Some(start) = module_call_start(init)
        {
            self.pending.insert(start, bindings_for_pattern(&it.id));
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Some(start) = module_call_start(&it.object) {
            let key = it.property.name.as_str();
            let kind = if key == "default" {
                ImportedKind::Default
            } else {
                ImportedKind::Named(key.to_owned())
            };
            self.pending.insert(start, vec![name(kind, None, false)]);
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        let names = self.pending.remove(&it.span.start);
        match static_string(&it.source) {
            Some(specifier) => self.info.imports.push(Import {
                specifier,
                kind: ImportKind::Dynamic,
                names: names.unwrap_or_else(escaping_namespace),
                span: self.ctx.span(it.span),
            }),
            None => self.info.unresolved_dynamic.push(self.ctx.span(it.span)),
        }
        walk::walk_import_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.visit_special_call(it);
        walk::walk_call_expression(self, it);
    }

    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) {
        if let TSModuleReference::ExternalModuleReference(reference) = &it.module_reference {
            let mut bound = name(ImportedKind::Namespace, Some(it.id.name.as_str()), false);
            bound.is_type = it.import_kind.is_type();
            self.info.imports.push(Import {
                specifier: crate::js_string(reference.expression.value),
                kind: if it.import_kind.is_type() {
                    ImportKind::TypeOnly
                } else {
                    ImportKind::Require
                },
                names: vec![bound],
                span: self.ctx.span(it.span),
            });
        }
        walk::walk_ts_import_equals_declaration(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let AssignmentTarget::StaticMemberExpression(m) = &it.left
            && is_cjs_member(m)
        {
            self.info.has_cjs_exports = true;
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_jsx_element(&mut self, it: &JSXElement<'a>) {
        self.info.has_jsx = true;
        walk::walk_jsx_element(self, it);
    }

    fn visit_jsx_fragment(&mut self, it: &JSXFragment<'a>) {
        self.info.has_jsx = true;
        walk::walk_jsx_fragment(self, it);
    }
}

impl Finder<'_, '_> {
    fn visit_special_call(&mut self, it: &CallExpression<'_>) {
        let first = it.arguments.first().and_then(Argument::as_expression);
        let span = self.ctx.span(it.span);
        match &it.callee {
            Expression::Identifier(id) if id.name.as_str() == "require" => match first.and_then(static_string) {
                Some(specifier) => {
                    let names = self.pending.remove(&it.span.start).unwrap_or_else(escaping_namespace);
                    self.info.imports.push(Import {
                        specifier,
                        kind: ImportKind::Require,
                        names,
                        span,
                    });
                }
                None if !it.arguments.is_empty() => self.info.unresolved_dynamic.push(span),
                None => {}
            },
            Expression::StaticMemberExpression(m) => {
                let property = m.property.name.as_str();
                if is_ident(&m.object, "require") && property == "resolve" {
                    match first.and_then(static_string) {
                        Some(specifier) => self.info.imports.push(Import {
                            specifier,
                            kind: ImportKind::Require,
                            names: Vec::new(),
                            span,
                        }),
                        None => self.info.unresolved_dynamic.push(span),
                    }
                } else if is_import_meta(&m.object) && (property == "glob" || property == "globEager") {
                    self.glob(first, span);
                }
            }
            _ => {}
        }
    }

    fn glob(&mut self, first: Option<&Expression<'_>>, span: skif_core::Span) {
        let patterns: Option<Vec<String>> = match first.map(Expression::get_inner_expression) {
            Some(Expression::ArrayExpression(a)) => a
                .elements
                .iter()
                .map(|e| e.as_expression().and_then(static_string))
                .collect(),
            Some(other) => static_string(other).map(|s| vec![s]),
            None => None,
        };
        match patterns {
            Some(patterns) => {
                for specifier in patterns {
                    self.info.imports.push(Import {
                        specifier,
                        kind: ImportKind::ImportMetaGlob,
                        names: escaping_namespace(),
                        span,
                    });
                }
            }
            None => self.info.unresolved_dynamic.push(span),
        }
    }
}
