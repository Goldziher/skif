//! The members and heritage of exported classes, and the property names a file reads and writes, for
//! `unused-class-members` (ADR 0042).
//!
//! Without type information a property read cannot be tied to the class it reads from, so a file records every
//! property name it reads on any object and the rule credits a member by its name. The class side records what the
//! rule needs to leave members alone: decorators, the superclass chain as far as the module can follow it, and the
//! members of the interfaces its `implements` clause names.

use std::collections::BTreeSet;

use oxc_ast::ast::{
    Argument, AssignmentExpression, AssignmentOperator, AssignmentTarget, AssignmentTargetPropertyIdentifier,
    AssignmentTargetPropertyProperty, BindingProperty, CallExpression, Class, ClassElement, ComputedMemberExpression,
    Declaration, ExportDefaultDeclarationKind, Expression, JSXMemberExpression, MethodDefinitionKind,
    PrivateFieldExpression, PrivateInExpression, Program, PropertyKey, Statement, StaticMemberExpression,
    TSAccessibility, TSInterfaceDeclaration, TSLiteral, TSLiteralType, TSQualifiedName, TSSignature, TSType,
    TSTypeLiteral, TSTypeName,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::Span as OxcSpan;
use skif_core::{HashMap, HashSet};

use crate::collect::{Ctx, ImportBinding, binding_ids, import_bindings, member_removal, mentions_word};
use crate::jsdoc::leading_tags;
use crate::model::{ClassMember, ClassMemberKind, ClassShape, ExportKind, Heritage, ImportedKind, ModuleInfo};

/// The top-level declarations of a module that a class's heritage can name.
struct TopLevel<'p, 'a> {
    classes: HashMap<String, &'p Class<'a>>,
    anonymous_default: Option<&'p Class<'a>>,
    interfaces: HashMap<String, Vec<&'p TSInterfaceDeclaration<'a>>>,
    type_literals: HashMap<String, &'p TSTypeLiteral<'a>>,
    imports: HashMap<String, ImportBinding>,
    /// Every other name a top-level declaration binds.
    bound: HashSet<String>,
}

impl<'p, 'a> TopLevel<'p, 'a> {
    fn of(program: &'p Program<'a>) -> Self {
        let mut top = Self {
            classes: HashMap::default(),
            anonymous_default: None,
            interfaces: HashMap::default(),
            type_literals: HashMap::default(),
            imports: import_bindings(program),
            bound: HashSet::default(),
        };
        for statement in &program.body {
            match statement {
                Statement::ExportDefaultDeclaration(e) => match &e.declaration {
                    ExportDefaultDeclarationKind::ClassDeclaration(class) => match &class.id {
                        Some(id) => {
                            top.classes.insert(id.name.as_str().to_owned(), class);
                        }
                        None => top.anonymous_default = Some(class),
                    },
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(interface) => {
                        top.add_interface(interface);
                    }
                    _ => {}
                },
                Statement::ExportDeclaration(e) => top.add(&e.declaration),
                other => {
                    if let Some(declaration) = other.as_declaration() {
                        top.add(declaration);
                    }
                }
            }
        }
        top
    }

    fn add(&mut self, declaration: &'p Declaration<'a>) {
        match declaration {
            Declaration::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    self.classes.insert(id.name.as_str().to_owned(), class);
                }
            }
            Declaration::TSInterfaceDeclaration(interface) => self.add_interface(interface),
            Declaration::TSTypeAliasDeclaration(alias) => {
                if let TSType::TSTypeLiteral(literal) = &alias.type_annotation {
                    self.type_literals.insert(alias.id.name.as_str().to_owned(), literal);
                }
            }
            Declaration::VariableDeclaration(variables) => {
                let mut ids = Vec::new();
                for declarator in &variables.declarations {
                    binding_ids(&declarator.id, &mut ids);
                }
                self.bound.extend(ids.into_iter().map(|(name, _)| name));
            }
            Declaration::FunctionDeclaration(function) => {
                if let Some(id) = &function.id {
                    self.bound.insert(id.name.as_str().to_owned());
                }
            }
            Declaration::TSEnumDeclaration(e) => {
                self.bound.insert(e.id.name.as_str().to_owned());
            }
            Declaration::TSNamespaceDeclaration(namespace) => {
                self.bound.insert(namespace.id.name.as_str().to_owned());
            }
            Declaration::TSImportEqualsDeclaration(import) => {
                self.bound.insert(import.id.name.as_str().to_owned());
            }
            _ => {}
        }
    }

    fn add_interface(&mut self, interface: &'p TSInterfaceDeclaration<'a>) {
        self.interfaces
            .entry(interface.id.name.as_str().to_owned())
            .or_default()
            .push(interface);
    }

    /// Where the superclass `expression` leaves the module, following the top-level classes it names; `None` when
    /// the chain ends in a class of the module that has no superclass.
    fn heritage(&self, expression: &Expression<'a>, seen: &mut HashSet<String>) -> Option<Heritage> {
        match expression.get_inner_expression() {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if let Some(binding) = self.imports.get(name) {
                    return Some(match &binding.kind {
                        ImportedKind::Named(export) => imported(binding, export),
                        ImportedKind::Default => imported(binding, "default"),
                        ImportedKind::Namespace => Heritage::Opaque,
                    });
                }
                if let Some(class) = self.classes.get(name) {
                    if !seen.insert(name.to_owned()) {
                        return Some(Heritage::Opaque);
                    }
                    return class
                        .heritage
                        .as_ref()
                        .and_then(|heritage| self.heritage(&heritage.expression, seen));
                }
                if self.bound.contains(name) || self.interfaces.contains_key(name) {
                    return Some(Heritage::Opaque);
                }
                Some(Heritage::Global(name.to_owned()))
            }
            Expression::StaticMemberExpression(member) => {
                let Expression::Identifier(object) = member.object.get_inner_expression() else {
                    return Some(Heritage::Opaque);
                };
                match self.imports.get(object.name.as_str()) {
                    Some(binding) if matches!(binding.kind, ImportedKind::Namespace | ImportedKind::Default) => {
                        Some(imported(binding, member.property.name.as_str()))
                    }
                    _ => Some(Heritage::Opaque),
                }
            }
            _ => Some(Heritage::Opaque),
        }
    }

    /// Adds the member names of the interface or object type alias `name` of the module, and of the local interfaces
    /// it extends, to `out`. `false` when a type in the chain is not declared in the module.
    fn interface_members(&self, name: &str, out: &mut BTreeSet<String>, seen: &mut HashSet<String>) -> bool {
        if !seen.insert(name.to_owned()) {
            return true;
        }
        if let Some(interfaces) = self.interfaces.get(name) {
            for interface in interfaces {
                signature_names(&interface.body.body, out);
                for parent in &interface.extends {
                    let TSTypeName::IdentifierReference(id) = &parent.type_name else {
                        return false;
                    };
                    if !self.interface_members(id.name.as_str(), out, seen) {
                        return false;
                    }
                }
            }
            return true;
        }
        if let Some(literal) = self.type_literals.get(name) {
            signature_names(&literal.members, out);
            return true;
        }
        false
    }

    fn shape(&self, ctx: &Ctx<'_>, class: &Class<'a>) -> ClassShape {
        let mut seen = HashSet::default();
        if let Some(id) = &class.id {
            seen.insert(id.name.as_str().to_owned());
        }
        let extends = class
            .heritage
            .as_ref()
            .and_then(|heritage| self.heritage(&heritage.expression, &mut seen));
        let mut implemented = BTreeSet::new();
        let mut implements_unknown = false;
        for clause in &class.implements {
            let known = match &clause.expression {
                TSTypeName::IdentifierReference(id) => {
                    self.interface_members(id.name.as_str(), &mut implemented, &mut HashSet::default())
                }
                _ => false,
            };
            implements_unknown |= !known;
        }
        ClassShape {
            members: members(ctx, class),
            decorated: !class.decorators.is_empty(),
            extends,
            implemented: implemented.into_iter().collect(),
            implements_unknown,
        }
    }
}

fn imported(binding: &ImportBinding, name: &str) -> Heritage {
    Heritage::Imported {
        specifier: binding.specifier.clone(),
        name: name.to_owned(),
    }
}

/// The names of the property and method signatures among `signatures`.
fn signature_names(signatures: &[TSSignature<'_>], out: &mut BTreeSet<String>) {
    for signature in signatures {
        let key = match signature {
            TSSignature::TSPropertySignature(property) => &property.key,
            TSSignature::TSMethodSignature(method) => &method.key,
            _ => continue,
        };
        if let Some(name) = key_name(key) {
            out.insert(name);
        }
    }
}

/// The name of a member key: an identifier, a literal (also computed, `['a']`), or a private name with its `#`.
fn key_name(key: &PropertyKey<'_>) -> Option<String> {
    if let Some(private) = key.private_name() {
        return Some(format!("#{}", private.as_str()));
    }
    key.static_name().map(|name| crate::js_string(name.as_js_str()))
}

/// One member of a class body, before removals are decided.
struct Declared {
    name: String,
    kind: ClassMemberKind,
    is_static: bool,
    decorated: bool,
    key_span: OxcSpan,
    span: OxcSpan,
}

fn declared(element: &ClassElement<'_>) -> Option<Declared> {
    let (key, kind, is_static, decorated, accessibility, span) = match element {
        ClassElement::MethodDefinition(method) => {
            let kind = match method.kind {
                MethodDefinitionKind::Constructor => return None,
                MethodDefinitionKind::Method => ClassMemberKind::Method,
                MethodDefinitionKind::Get => ClassMemberKind::Getter,
                MethodDefinitionKind::Set => ClassMemberKind::Setter,
            };
            let decorated = !method.decorators.is_empty();
            (
                &method.key,
                kind,
                method.r#static,
                decorated,
                method.accessibility,
                method.span,
            )
        }
        ClassElement::PropertyDefinition(property) => {
            let decorated = !property.decorators.is_empty();
            let kind = ClassMemberKind::Property;
            (
                &property.key,
                kind,
                property.r#static,
                decorated,
                property.accessibility,
                property.span,
            )
        }
        ClassElement::AccessorProperty(property) => {
            let decorated = !property.decorators.is_empty();
            let kind = ClassMemberKind::Property;
            (
                &property.key,
                kind,
                property.r#static,
                decorated,
                property.accessibility,
                property.span,
            )
        }
        ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => return None,
    };
    if accessibility == Some(TSAccessibility::Private) {
        return None;
    }
    Some(Declared {
        name: key_name(key)?,
        kind,
        is_static,
        decorated,
        key_span: oxc_span::GetSpan::span(key),
        span,
    })
}

/// The members of `class` as [`ClassMember`]s. A name declared more than once with the same kind and placement (the
/// signatures of an overloaded method) is one member, which gets no removal.
fn members(ctx: &Ctx<'_>, class: &Class<'_>) -> Vec<ClassMember> {
    let declared: Vec<Declared> = class.body.body.iter().filter_map(declared).collect();
    let body = class.body.span;
    let mut out: Vec<ClassMember> = Vec::with_capacity(declared.len());
    for (index, member) in declared.iter().enumerate() {
        let same = |other: &Declared| {
            other.name == member.name && other.kind == member.kind && other.is_static == member.is_static
        };
        if declared[..index].iter().any(same) {
            continue;
        }
        let repeated = declared[index + 1..].iter().any(same);
        let before = &ctx.src[body.start as usize..member.span.start as usize];
        let after = &ctx.src[member.span.end as usize..body.end as usize];
        let mentioned = mentions_word(before, bare(&member.name)) || mentions_word(after, bare(&member.name));
        out.push(ClassMember {
            removal: if repeated || mentioned || member.decorated {
                None
            } else {
                member_removal(ctx, member.span, false)
            },
            name: member.name.clone(),
            kind: member.kind,
            is_static: member.is_static,
            decorated: member.decorated,
            span: ctx.span(member.key_span),
            tags: leading_tags(ctx.src, member.span.start as usize),
        });
    }
    out
}

/// A member name without the `#` of a private name, as other code of the class spells it after `this.#`.
fn bare(name: &str) -> &str {
    name.strip_prefix('#').unwrap_or(name)
}

/// Sets [`crate::model::Export::class`] on the exports of a top-level class declaration.
pub(crate) fn mark(ctx: &Ctx<'_>, program: &Program<'_>, info: &mut ModuleInfo) {
    let exports_class = info
        .exports
        .iter()
        .any(|export| matches!(export.kind, ExportKind::Class | ExportKind::Default) && export.from.is_none());
    if !exports_class {
        return;
    }
    let top = TopLevel::of(program);
    if top.classes.is_empty() && top.anonymous_default.is_none() {
        return;
    }
    for export in &mut info.exports {
        if export.from.is_some() {
            continue;
        }
        let class = match (export.kind, export.local_name.as_deref()) {
            (ExportKind::Class | ExportKind::Default, Some(local)) => top.classes.get(local).copied(),
            (ExportKind::Default, None) => top.anonymous_default,
            _ => None,
        };
        if let Some(class) = class.filter(|class| !class.declare) {
            export.class = Some(top.shape(ctx, class));
        }
    }
}

/// Records the property names `program` reads and writes in [`ModuleInfo::property_reads`] and
/// [`ModuleInfo::property_writes`].
pub(crate) fn property_names(program: &Program<'_>, info: &mut ModuleInfo) {
    let mut names = PropertyNames::default();
    names.visit_program(program);
    info.add_property_reads(names.reads);
    info.property_writes.extend(names.writes);
    info.property_writes.sort();
    info.property_writes.dedup();
}

/// Every name read as `.name` in `text`, which is markup or code the parser does not read: `x.name`, `x?.name`,
/// `x!.name`. Sorted, without duplicates.
pub(crate) fn lexical_reads(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let is_start = |b: u8| b.is_ascii_alphabetic() || b == b'_' || b == b'$';
    let is_part = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut found = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'.' {
            i += 1;
            continue;
        }
        let mut start = i + 1;
        while start < bytes.len() && matches!(bytes[start], b' ' | b'\t' | b'\n' | b'\r') {
            start += 1;
        }
        if start >= bytes.len() || !is_start(bytes[start]) {
            i += 1;
            continue;
        }
        let mut end = start;
        while end < bytes.len() && is_part(bytes[end]) {
            end += 1;
        }
        found.insert(text[start..end].to_owned());
        i = end;
    }
    found.into_iter().collect()
}

#[derive(Default)]
struct PropertyNames {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
}

impl PropertyNames {
    fn read(&mut self, name: &str) {
        if !self.reads.contains(name) {
            self.reads.insert(name.to_owned());
        }
    }
}

/// The value of a string literal or a template literal without substitutions.
fn literal_name(expression: &Expression<'_>) -> Option<String> {
    match expression.get_inner_expression() {
        Expression::StringLiteral(literal) => Some(crate::js_string(literal.value)),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            template.quasis.first().map(|quasi| quasi.value.raw.as_str().to_owned())
        }
        _ => None,
    }
}

impl<'a> Visit<'a> for PropertyNames {
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if it.operator == AssignmentOperator::Assign {
            let written = match &it.left {
                AssignmentTarget::StaticMemberExpression(member) => {
                    Some((member.property.name.as_str().to_owned(), &member.object))
                }
                AssignmentTarget::ComputedMemberExpression(member) => {
                    literal_name(&member.expression).map(|name| (name, &member.object))
                }
                AssignmentTarget::PrivateFieldExpression(member) => {
                    Some((format!("#{}", member.field.name.as_str()), &member.object))
                }
                _ => None,
            };
            if let Some((name, object)) = written {
                self.writes.insert(name);
                self.visit_expression(object);
                self.visit_expression(&it.right);
                return;
            }
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.read(it.property.name.as_str());
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Some(name) = literal_name(&it.expression) {
            self.read(&name);
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_private_field_expression(&mut self, it: &PrivateFieldExpression<'a>) {
        self.read(&format!("#{}", it.field.name.as_str()));
        walk::walk_private_field_expression(self, it);
    }

    fn visit_private_in_expression(&mut self, it: &PrivateInExpression<'a>) {
        self.read(&format!("#{}", it.left.name.as_str()));
        walk::walk_private_in_expression(self, it);
    }

    fn visit_binding_property(&mut self, it: &BindingProperty<'a>) {
        if let Some(name) = key_name(&it.key) {
            self.read(&name);
        }
        walk::walk_binding_property(self, it);
    }

    fn visit_assignment_target_property_identifier(&mut self, it: &AssignmentTargetPropertyIdentifier<'a>) {
        self.read(it.binding.name.as_str());
        walk::walk_assignment_target_property_identifier(self, it);
    }

    fn visit_assignment_target_property_property(&mut self, it: &AssignmentTargetPropertyProperty<'a>) {
        if let Some(name) = key_name(&it.name) {
            self.read(&name);
        }
        walk::walk_assignment_target_property_property(self, it);
    }

    fn visit_jsx_member_expression(&mut self, it: &JSXMemberExpression<'a>) {
        self.read(it.property.name.as_str());
        walk::walk_jsx_member_expression(self, it);
    }

    fn visit_ts_qualified_name(&mut self, it: &TSQualifiedName<'a>) {
        self.read(it.right.name.as_str());
        walk::walk_ts_qualified_name(self, it);
    }

    fn visit_ts_literal_type(&mut self, it: &TSLiteralType<'a>) {
        if let TSLiteral::StringLiteral(literal) = &it.literal {
            self.read(&crate::js_string(literal.value));
        }
        walk::walk_ts_literal_type(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        for argument in &it.arguments {
            if let Argument::StringLiteral(literal) = argument {
                self.read(&crate::js_string(literal.value));
            }
        }
        walk::walk_call_expression(self, it);
    }
}
