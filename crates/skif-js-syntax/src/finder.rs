//! Nested module references: `require`, dynamic `import()`, `import.meta.glob`, `import x = require()`, test-runner
//! mocks, `CommonJS` exports, JSX and the template and stylesheet files of an Angular or Stencil `@Component`.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrayPattern, AssignmentExpression, AssignmentOperator, AssignmentTarget,
    BindingPattern, CallExpression, ComputedMemberExpression, Decorator, Expression, FormalParameters,
    IdentifierReference, ImportExpression, JSXElement, JSXFragment, ObjectPropertyKind, Program, Statement,
    StaticMemberExpression, TSAsExpression, TSExternalModuleDeclaration, TSImportEqualsDeclaration, TSImportType,
    TSImportTypeQualifier, TSIndexedAccessType, TSLiteral, TSModuleReference, TSType, TSTypeQueryExprName,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;
use skif_core::{HashMap, HashSet};

use crate::collect::{Ctx, collect_import, imported_kind};
use crate::model::{
    Export, ExportKind, Import, ImportKind, ImportedKind, ImportedName, MockCall, ModuleInfo, is_source_query,
};

pub(crate) struct Finder<'c, 's> {
    ctx: &'c Ctx<'s>,
    info: &'c mut ModuleInfo,
    /// Bindings for a call or `import()` expression, keyed by the expression's start offset.
    pending: HashMap<u32, Vec<ImportedName>>,
    /// Start offsets of `typeof import("x")` types that only type the result of the `require("x")` they follow.
    redundant_types: HashSet<u32>,
    /// The export a `typeof import("x")["A"]` reads, by the start offset of its import type.
    indexed_types: HashMap<u32, String>,
    /// The file has top-level `import` or `export`, so its `declare module` blocks augment rather than declare.
    module_file: bool,
    /// Local names of the `Component` decorator imported from `@angular/core` or `@stencil/core`.
    component_decorators: HashSet<String>,
    /// Start offsets of the `import("x")` expressions a mock names (`vi.mock(import("x"))`), which are not imports.
    mocked_imports: HashSet<u32>,
    /// The names of `info.exports`, gathered at the first `CommonJS` export, which keeps them up to date.
    export_names: Option<HashSet<String>>,
    /// The properties of `module.exports = { a, b }`: the binding each names and the span of its identifier.
    cjs_shorthand: Vec<(String, oxc_span::Span)>,
}

impl Finder<'_, '_> {
    /// Records the named exports of `exports.x = …`, `module.exports.x = …` and `module.exports = { a, b }` (all
    /// properties shorthand; any other object is one anonymous value).
    fn cjs_exports(&mut self, target: &StaticMemberExpression<'_>, right: &Expression<'_>) {
        if is_module_exports(target) {
            let Expression::ObjectExpression(object) = right.get_inner_expression() else {
                return;
            };
            let shorthand: Option<Vec<_>> = object
                .properties
                .iter()
                .map(|property| match property {
                    ObjectPropertyKind::ObjectProperty(p) if p.shorthand => Some((
                        crate::js_string(p.key.static_name()?.as_js_str()),
                        p.key.span(),
                        p.value.span(),
                    )),
                    _ => None,
                })
                .collect();
            for (name, key, value) in shorthand.filter(|names| !names.is_empty()).unwrap_or_default() {
                self.cjs_export(&name, key);
                self.cjs_shorthand.push((name, value));
            }
        } else {
            self.cjs_export(target.property.name.as_str(), target.property.span);
        }
    }

    fn cjs_export(&mut self, name: &str, span: oxc_span::Span) {
        let names = self
            .export_names
            .get_or_insert_with(|| self.info.exports.iter().map(|export| export.name.clone()).collect());
        if names.contains(name) {
            return;
        }
        names.insert(name.to_owned());
        let mut export = Export::bare(name.to_owned(), ExportKind::Value, self.ctx.span(span));
        export.local_name = None;
        self.info.exports.push(export);
    }
}

impl<'c, 's> Finder<'c, 's> {
    pub(crate) fn new(ctx: &'c Ctx<'s>, info: &'c mut ModuleInfo, program: &Program<'_>) -> Self {
        Self {
            pending: HashMap::default(),
            redundant_types: HashSet::default(),
            indexed_types: HashMap::default(),
            module_file: is_module_file(program),
            component_decorators: component_decorator_names(info),
            mocked_imports: HashSet::default(),
            export_names: None,
            cjs_shorthand: Vec::new(),
            ctx,
            info,
        }
    }

    /// The properties of the `module.exports = { a, b }` objects the walk found: the binding each names and the span
    /// of its identifier, which exports the binding without reading it.
    pub(crate) fn into_cjs_shorthand(self) -> Vec<(String, oxc_span::Span)> {
        self.cjs_shorthand
    }
}

/// The local names the `Component` decorator of Angular or Stencil is imported under.
fn component_decorator_names(info: &ModuleInfo) -> HashSet<String> {
    info.imports
        .iter()
        .filter(|import| matches!(import.specifier.as_str(), "@angular/core" | "@stencil/core"))
        .flat_map(|import| &import.names)
        .filter(|name| name.kind == ImportedKind::Named("Component".to_owned()))
        .filter_map(|name| name.local.clone())
        .collect()
}

/// A component resource URL as a specifier: it is relative to the component file even without `./`.
fn component_resource(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.contains("://") || url.starts_with("data:") {
        return None;
    }
    Some(if url.starts_with('.') || url.starts_with('/') {
        url.to_owned()
    } else {
        format!("./{url}")
    })
}

fn name(kind: ImportedKind, local: Option<&str>, escapes: bool) -> ImportedName {
    ImportedName {
        kind,
        local: local.map(str::to_string),
        is_type: false,
        accessed_members: Vec::new(),
        nested_members: BTreeMap::new(),
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

/// The glob a computed relative specifier can match: `./commands/${name}.js` reads `./commands/*.js`. `None` for
/// anything else, whose target nothing in the source narrows down.
fn template_glob(expr: &Expression<'_>) -> Option<String> {
    let Expression::TemplateLiteral(template) = expr.get_inner_expression() else {
        return None;
    };
    if template.expressions.is_empty() {
        return None;
    }
    let parts: Vec<String> = template
        .quasis
        .iter()
        .map(|quasi| {
            quasi
                .value
                .cooked
                .map_or_else(|| quasi.value.raw.as_str().to_owned(), crate::js_string)
        })
        .collect();
    let relative = parts
        .first()
        .is_some_and(|first| first.starts_with("./") || first.starts_with("../"));
    relative.then(|| parts.join("*"))
}

/// The test-runner call (`vi.mock`, `jest.requireActual`) that `member` is, if any.
fn mock_call(member: &StaticMemberExpression<'_>) -> Option<MockCall> {
    let Expression::Identifier(object) = &member.object else {
        return None;
    };
    MockCall::find(object.name.as_str(), member.property.name.as_str())
}

/// The module a mock names: a string, or Vitest's typed `vi.mock(import("x"))`, whose `import()` is then claimed by
/// the mock rather than recorded as a dynamic import.
fn mock_specifier(first: &Expression<'_>) -> Option<(String, Option<u32>)> {
    match first.get_inner_expression() {
        Expression::ImportExpression(import) => static_string(&import.source).map(|s| (s, Some(import.span.start))),
        other => static_string(other).map(|s| (s, None)),
    }
}

/// `jest.mock("x", factory, { virtual: true })` mocks a module that does not exist.
fn is_virtual_mock(call: &CallExpression<'_>) -> bool {
    let Some(Expression::ObjectExpression(options)) = call.arguments.get(2).and_then(Argument::as_expression) else {
        return false;
    };
    options.properties.iter().any(|property| {
        matches!(
            property,
            ObjectPropertyKind::ObjectProperty(p)
                if p.key.is_specific_static_name("virtual")
                    && matches!(&p.value, Expression::BooleanLiteral(b) if b.value)
        )
    })
}

fn is_ident(expr: &Expression<'_>, wanted: &str) -> bool {
    matches!(expr, Expression::Identifier(id) if id.name.as_str() == wanted)
}

fn is_import_meta(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::ImportMeta(_))
}

/// Whether `call` resolves a module specifier to its path: `require.resolve(…)`, `import.meta.resolve(…)` or the
/// `resolve` of a `createRequire(…)` call, also behind parentheses or a TypeScript assertion
/// (`(require as NodeRequire).resolve(…)`).
pub(crate) fn is_module_resolve(call: &CallExpression<'_>) -> bool {
    let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
        return false;
    };
    member.property.name.as_str() == "resolve"
        && match member.object.get_inner_expression() {
            Expression::Identifier(object) => object.name.as_str() == "require",
            Expression::ImportMeta(_) => true,
            Expression::CallExpression(factory) => is_ident(factory.callee.get_inner_expression(), "createRequire"),
            _ => false,
        }
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
pub(crate) fn module_call_start(expr: &Expression<'_>) -> Option<u32> {
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

/// The bindings `const [a, { b }] = await Promise.all([import("x"), import("y")])` takes from each `import()`, keyed
/// by the start of the import: the element of the array pattern at the import's position, read like the pattern of
/// `const a = await import("x")`. An import after a spread element, or without an element of the pattern at its
/// position (a hole, a shorter pattern, a rest element), gets no entry and stays used as a whole.
fn promise_all_bindings(init: &Expression<'_>, pattern: &ArrayPattern<'_>) -> Vec<(u32, Vec<ImportedName>)> {
    promise_all_imports(init, pattern)
        .into_iter()
        .map(|(start, binding)| (start, bindings_for_pattern(binding)))
        .collect()
}

/// The element of the array pattern of `const [a, { b }] = await Promise.all([import("x"), import("y")])` each
/// `import()` binds, by the start of the import. See [`promise_all_bindings`].
pub(crate) fn promise_all_imports<'p, 'a>(
    init: &'p Expression<'a>,
    pattern: &'p ArrayPattern<'a>,
) -> Vec<(u32, &'p BindingPattern<'a>)> {
    let inner = match init.get_inner_expression() {
        Expression::AwaitExpression(a) => a.argument.get_inner_expression(),
        other => other,
    };
    let Expression::CallExpression(call) = inner else {
        return Vec::new();
    };
    let Expression::StaticMemberExpression(callee) = call.callee.get_inner_expression() else {
        return Vec::new();
    };
    if !is_ident(&callee.object, "Promise") || callee.property.name.as_str() != "all" {
        return Vec::new();
    }
    let Some(Argument::ArrayExpression(array)) = call.arguments.first() else {
        return Vec::new();
    };
    array
        .elements
        .iter()
        .take_while(|element| !matches!(element, ArrayExpressionElement::SpreadElement(_)))
        .zip(&pattern.elements)
        .filter_map(|(element, binding)| {
            let binding = binding.as_ref()?;
            let Expression::ImportExpression(import) = element.as_expression()?.get_inner_expression() else {
                return None;
            };
            Some((import.span.start, binding))
        })
        .collect()
}

/// The export a qualified import type names and the member it reads of that export: `A` and `None` for
/// `import("x").A`, `A` and `Some("B")` for `import("x").A.B` (and `.A.B.C`).
fn qualifier_parts<'q>(qualifier: &'q TSImportTypeQualifier<'_>) -> (&'q str, Option<&'q str>) {
    match qualifier {
        TSImportTypeQualifier::Identifier(i) => (i.name.as_str(), None),
        TSImportTypeQualifier::QualifiedName(n) => match &n.left {
            TSImportTypeQualifier::Identifier(i) => (i.name.as_str(), Some(n.right.name.as_str())),
            left @ TSImportTypeQualifier::QualifiedName(_) => qualifier_parts(left),
        },
    }
}

impl Finder<'_, '_> {
    /// Records `templateUrl`, `styleUrl` and `styleUrls` of an Angular or Stencil `@Component({ … })` as
    /// side-effect imports, so the stylesheets are followed and their packages count as used. Stencil's `styleUrls`
    /// may map modes to URLs. Its `assetsDirs` name directories copied as they are, not modules, and are not followed.
    fn component_resources(&mut self, decorator: &Decorator<'_>) {
        let Expression::CallExpression(call) = &decorator.expression else {
            return;
        };
        let Expression::Identifier(callee) = &call.callee else {
            return;
        };
        if !self.component_decorators.contains(callee.name.as_str()) {
            return;
        }
        let Some(Argument::ObjectExpression(options)) = call.arguments.first() else {
            return;
        };
        let mut found = Vec::new();
        for property in &options.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            let Some(key) = property.key.static_name() else {
                continue;
            };
            match crate::js_string(key.as_js_str()).as_str() {
                "templateUrl" | "styleUrl" => found.push(&property.value),
                "styleUrls" => match property.value.get_inner_expression() {
                    Expression::ObjectExpression(modes) => {
                        // Stencil's styles per mode: `{ ios: 'a.ios.css', md: ['a.md.css'] }`.
                        for mode in &modes.properties {
                            if let ObjectPropertyKind::ObjectProperty(mode) = mode {
                                style_urls(&mode.value, &mut found);
                            }
                        }
                    }
                    value => style_urls(value, &mut found),
                },
                _ => {}
            }
        }
        for value in found {
            if let Some(specifier) = static_string(value).as_deref().and_then(component_resource) {
                self.info.imports.push(Import {
                    specifier,
                    kind: ImportKind::SideEffect,
                    names: Vec::new(),
                    call: None,
                    span: self.ctx.span(value.span()),
                });
            }
        }
    }
}

/// Collects the URLs of a `styleUrls` array, or of a single URL as Stencil allows for one mode.
fn style_urls<'b>(value: &'b Expression<'_>, found: &mut Vec<&'b Expression<'b>>) {
    match value.get_inner_expression() {
        Expression::ArrayExpression(array) => {
            found.extend(array.elements.iter().filter_map(|element| match element {
                ArrayExpressionElement::SpreadElement(_) | ArrayExpressionElement::Elision(_) => None,
                element => element.as_expression(),
            }));
        }
        _ => found.push(value),
    }
}

impl<'a> Visit<'a> for Finder<'_, '_> {
    fn visit_decorator(&mut self, it: &Decorator<'a>) {
        if !self.component_decorators.is_empty() {
            self.component_resources(it);
        }
        walk::walk_decorator(self, it);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init {
            if let Some(start) = module_call_start(init) {
                self.pending.insert(start, bindings_for_pattern(&it.id));
            } else if let BindingPattern::ArrayPattern(pattern) = &it.id {
                self.pending.extend(promise_all_bindings(init, pattern));
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        let promise_method = matches!(it.property.name.as_str(), "then" | "catch" | "finally")
            && matches!(it.object.get_inner_expression(), Expression::ImportExpression(_));
        if !promise_method && let Some(start) = module_call_start(&it.object) {
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
        if self.mocked_imports.remove(&it.span.start) {
            walk::walk_import_expression(self, it);
            return;
        }
        let names = self.pending.remove(&it.span.start);
        let span = self.ctx.span(it.span);
        if let Some(specifier) = static_string(&it.source) {
            self.info.imports.push(Import {
                specifier,
                kind: ImportKind::Dynamic,
                names: names.unwrap_or_else(escaping_namespace),
                call: None,
                span,
            });
        } else {
            self.computed_specifier(Some(&it.source), span);
        }
        walk::walk_import_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.then_callback(it);
        self.visit_special_call(it);
        walk::walk_call_expression(self, it);
    }

    fn visit_ts_external_module_declaration(&mut self, it: &TSExternalModuleDeclaration<'a>) {
        let name = crate::js_string(it.id.value);
        if self.module_file {
            self.info.augmented_modules.push(name);
        } else {
            self.info.ambient_modules.push(name);
        }
        // `declare module "x" { import m from "y"; }` needs `y` as much as a top-level import does.
        for statement in it.body.iter().flat_map(|body| &body.body) {
            if let Statement::ImportDeclaration(declaration) = statement {
                collect_import(self.ctx, declaration, self.info);
            }
        }
        walk::walk_ts_external_module_declaration(self, it);
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
                call: None,
                span: self.ctx.span(it.span),
            });
        }
        walk::walk_ts_import_equals_declaration(self, it);
    }

    // `require("x") as typeof import("x")` loads the module once; the bindings of the `require` name what is used, and
    // the assertion must not count as a use of the whole module.
    fn visit_ts_as_expression(&mut self, it: &TSAsExpression<'a>) {
        if module_call_start(&it.expression).is_some()
            && let TSType::TSTypeQuery(query) = &it.type_annotation
            && let TSTypeQueryExprName::TSImportType(import) = &query.expr_name
            && import.qualifier.is_none()
        {
            self.redundant_types.insert(import.span.start);
        }
        walk::walk_ts_as_expression(self, it);
    }

    fn visit_ts_indexed_access_type(&mut self, it: &TSIndexedAccessType<'a>) {
        let import: Option<&TSImportType<'a>> = match &it.object_type {
            TSType::TSImportType(import) => Some(import),
            TSType::TSTypeQuery(query) => match &query.expr_name {
                TSTypeQueryExprName::TSImportType(import) => Some(import),
                _ => None,
            },
            _ => None,
        };
        if let Some(import) = import
            && import.qualifier.is_none()
            && let TSType::TSLiteralType(literal) = &it.index_type
            && let TSLiteral::StringLiteral(export) = &literal.literal
        {
            self.indexed_types
                .insert(import.span.start, crate::js_string(export.value));
        }
        walk::walk_ts_indexed_access_type(self, it);
    }

    fn visit_ts_import_type(&mut self, it: &TSImportType<'a>) {
        if self.redundant_types.contains(&it.span.start) {
            walk::walk_ts_import_type(self, it);
            return;
        }
        // `import("x").A` uses export `A` as a whole and `import("x").A.B` only its member `B`. A bare `import("x")`
        // names the module's type without reading any export of it, so it credits none: the module is reached, like
        // a side-effect import, and its exports stay to the members read elsewhere (knip credits none either).
        let mut bound = match &it.qualifier {
            Some(qualifier) => {
                let (export, member) = qualifier_parts(qualifier);
                let mut bound = name(imported_kind(export.to_string()), None, member.is_none());
                bound.accessed_members.extend(member.map(str::to_string));
                bound
            }
            None => match self.indexed_types.get(&it.span.start) {
                // `typeof import("x")["A"]`, as unplugin-auto-import generates, reads export `A` as a whole.
                Some(export) => name(imported_kind(export.clone()), None, true),
                None => name(ImportedKind::Namespace, None, false),
            },
        };
        bound.is_type = true;
        self.info.imports.push(Import {
            specifier: crate::js_string(it.source.value),
            kind: ImportKind::TypeOnly,
            names: vec![bound],
            call: None,
            span: self.ctx.span(it.span),
        });
        walk::walk_ts_import_type(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let AssignmentTarget::StaticMemberExpression(m) = &it.left
            && is_cjs_member(m)
        {
            self.info.has_cjs_exports = true;
            if it.operator == AssignmentOperator::Assign {
                self.cjs_exports(m, &it.right);
            }
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

/// Reads of the module namespace `target` inside a `.then` callback.
struct NamespaceReads<'n> {
    target: &'n str,
    members: Vec<String>,
    escapes: bool,
}

impl<'a> Visit<'a> for NamespaceReads<'_> {
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if is_ident(&it.object, self.target) {
            self.members.push(it.property.name.as_str().to_owned());
        } else {
            walk::walk_static_member_expression(self, it);
        }
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if is_ident(&it.object, self.target) {
            match static_string(&it.expression) {
                Some(member) => self.members.push(member),
                None => self.escapes = true,
            }
            self.visit_expression(&it.expression);
        } else {
            walk::walk_computed_member_expression(self, it);
        }
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if it.name.as_str() == self.target {
            self.escapes = true;
        }
    }
}

fn namespace_reads(target: &str, visit: impl FnOnce(&mut NamespaceReads<'_>)) -> Vec<ImportedName> {
    let mut reads = NamespaceReads {
        target,
        members: Vec::new(),
        escapes: false,
    };
    visit(&mut reads);
    if reads.escapes || reads.members.is_empty() {
        return escaping_namespace();
    }
    reads
        .members
        .into_iter()
        .map(|member| {
            let kind = if member == "default" {
                ImportedKind::Default
            } else {
                ImportedKind::Named(member)
            };
            name(kind, None, false)
        })
        .collect()
}

/// What the first parameter of a `.then` callback reads from the module: `m => m.Page`, `({ Page }) => Page`.
fn callback_names(params: &FormalParameters<'_>, visit: impl FnOnce(&mut NamespaceReads<'_>)) -> Vec<ImportedName> {
    match params.items.first().map(|param| &param.pattern) {
        Some(BindingPattern::BindingIdentifier(b)) => namespace_reads(b.name.as_str(), visit),
        Some(pattern @ BindingPattern::ObjectPattern(_)) => bindings_for_pattern(pattern),
        _ => escaping_namespace(),
    }
}

/// The `import()` of `import('./x').then(cb)`.
pub(crate) fn then_on_import<'p, 'a>(call: &'p CallExpression<'a>) -> Option<&'p ImportExpression<'a>> {
    let Expression::StaticMemberExpression(member) = &call.callee else {
        return None;
    };
    if member.property.name.as_str() != "then" {
        return None;
    }
    match member.object.get_inner_expression() {
        Expression::ImportExpression(import) => Some(&**import),
        _ => None,
    }
}

impl Finder<'_, '_> {
    /// `import('./x').then(cb)`: the exports used are those `cb` reads from its parameter, not a member named `then`.
    fn then_callback(&mut self, it: &CallExpression<'_>) {
        let Some(import) = then_on_import(it) else {
            return;
        };
        let names = match it.arguments.first() {
            Some(Argument::ArrowFunctionExpression(arrow)) => callback_names(&arrow.params, |reads| {
                reads.visit_arrow_function_body(&arrow.body);
            }),
            Some(Argument::FunctionExpression(function)) => callback_names(&function.params, |reads| {
                if let Some(body) = &function.body {
                    reads.visit_function_body(body);
                }
            }),
            _ => return,
        };
        self.pending.insert(import.span.start, names);
    }

    fn visit_special_call(&mut self, it: &CallExpression<'_>) {
        let first = it.arguments.first().and_then(Argument::as_expression);
        // Most calls are none of these, so the span is mapped to lines only for one that is recorded.
        let ctx = self.ctx;
        let span = || ctx.span(it.span);
        if is_module_resolve(it) {
            self.resolve_call(first, span());
            return;
        }
        match &it.callee {
            Expression::Identifier(id) if id.name.as_str() == "require" => match first.and_then(static_string) {
                Some(specifier) => {
                    let names = self.pending.remove(&it.span.start).unwrap_or_else(escaping_namespace);
                    self.info.imports.push(Import {
                        specifier,
                        kind: ImportKind::Require,
                        names,
                        call: None,
                        span: span(),
                    });
                }
                None if !it.arguments.is_empty() => self.computed_specifier(first, span()),
                None => {}
            },
            Expression::StaticMemberExpression(m) => {
                let property = m.property.name.as_str();
                if is_import_meta(&m.object) && (property == "glob" || property == "globEager") {
                    let options = it.arguments.get(1).and_then(Argument::as_expression);
                    self.glob(first, options, span());
                } else if let Some(call) = mock_call(m)
                    && !is_virtual_mock(it)
                    && let Some((specifier, import)) = first.and_then(mock_specifier)
                    && !specifier.is_empty()
                {
                    if let Some(start) = import {
                        self.mocked_imports.insert(start);
                    }
                    self.info.imports.push(Import {
                        specifier,
                        kind: ImportKind::Mock,
                        names: Vec::new(),
                        call: Some(call),
                        span: span(),
                    });
                }
            }
            _ => {}
        }
    }

    /// A resolve call ([`is_module_resolve`]): it names the module by a constant specifier, or by a computed one as a
    /// `require` does.
    fn resolve_call(&mut self, specifier: Option<&Expression<'_>>, span: skif_core::Span) {
        match specifier.and_then(static_string) {
            Some(specifier) => self.info.imports.push(Import {
                specifier,
                kind: ImportKind::Resolve,
                names: Vec::new(),
                call: None,
                span,
            }),
            None => self.computed_specifier(specifier, span),
        }
    }

    /// A `require`, `import()` or resolve call whose specifier is not a constant: followed as a glob when the source
    /// narrows it to a directory, and every matching module counts as read in full. Otherwise it is an unknown target.
    fn computed_specifier(&mut self, specifier: Option<&Expression<'_>>, span: skif_core::Span) {
        if let Some(specifier) = specifier.and_then(template_glob) {
            self.info.imports.push(Import {
                specifier,
                kind: ImportKind::ImportMetaGlob,
                names: escaping_namespace(),
                call: None,
                span,
            });
        } else {
            self.info.unresolved_dynamic.push(span);
        }
    }

    fn glob(&mut self, first: Option<&Expression<'_>>, options: Option<&Expression<'_>>, span: skif_core::Span) {
        let patterns: Option<Vec<String>> = match first.map(Expression::get_inner_expression) {
            Some(Expression::ArrayExpression(a)) => a
                .elements
                .iter()
                .map(|e| e.as_expression().and_then(static_string))
                .collect(),
            Some(other) => static_string(other).map(|s| vec![s]),
            None => None,
        };
        let kind = if glob_reads_source(options) {
            ImportKind::SourceGlob
        } else {
            ImportKind::ImportMetaGlob
        };
        match patterns {
            Some(patterns) => {
                for specifier in patterns {
                    self.info.imports.push(Import {
                        specifier,
                        kind,
                        names: escaping_namespace(),
                        call: None,
                        span,
                    });
                }
            }
            None => self.info.unresolved_dynamic.push(span),
        }
    }
}

/// Whether the options of an `import.meta.glob` read the matched files as text: the query Vite builds from them
/// (`query` as a string or an object, or the legacy `as`) passes [`is_source_query`]. Options that are not a static
/// object answer `false`, so the matches stay modules.
fn glob_reads_source(options: Option<&Expression<'_>>) -> bool {
    let Some(Expression::ObjectExpression(options)) = options.map(Expression::get_inner_expression) else {
        return false;
    };
    let mut query = None;
    for property in &options.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return false;
        };
        let Some(key) = property.key.static_name() else {
            return false;
        };
        if key == "query" || key == "as" {
            match static_query(&property.value) {
                Some(value) => query = Some(value),
                None => return false,
            }
        }
    }
    query.is_some_and(|query| is_source_query(&query))
}

/// The query string Vite builds from a static glob `query` option: a string as written, an object as
/// `key=value` parameters, and a key alone when its value is the empty string.
fn static_query(value: &Expression<'_>) -> Option<String> {
    let Expression::ObjectExpression(object) = value.get_inner_expression() else {
        return static_string(value);
    };
    let mut parameters = Vec::new();
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        let key = crate::js_string(property.key.static_name()?.as_js_str());
        let value = match property.value.get_inner_expression() {
            Expression::BooleanLiteral(b) => b.value.to_string(),
            Expression::NumericLiteral(n) => n.value.to_string(),
            other => static_string(other)?,
        };
        if value.is_empty() {
            parameters.push(key);
        } else {
            parameters.push(format!("{key}={value}"));
        }
    }
    Some(parameters.join("&"))
}

/// TypeScript's test for a module file: a top-level `import`, `export` or `import x = require(…)`. In a script,
/// `declare module 'x'` declares `x`; in a module file it augments the `x` that is already there.
fn is_module_file(program: &Program<'_>) -> bool {
    program.body.iter().any(|statement| {
        statement.is_module_declaration()
            || matches!(
                statement,
                Statement::TSImportEqualsDeclaration(declaration)
                    if matches!(declaration.module_reference, TSModuleReference::ExternalModuleReference(_))
            )
    })
}
