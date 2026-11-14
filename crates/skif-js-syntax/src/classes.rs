//! The members and heritage of exported classes, and the property names a file reads and writes, for
//! `unused-class-members` (ADR 0042).
//!
//! Without type information a property read cannot be tied to the class it reads from, so a file records every
//! property name it reads on any object and the rule credits a member by its name. The class side records what the
//! rule needs to leave members alone: decorators, the superclass chain as far as the module can follow it, and the
//! members of the interfaces its `implements` clause names.
//!
//! An element access whose key is a constant (`x[KEY]`, `x[Flags.SKIP]`) reads the string the constant holds: the
//! constants of the module resolve here, imported ones are recorded for the module graph to resolve.

use std::collections::BTreeSet;

use oxc_ast::ast::{
    Argument, AssignmentExpression, AssignmentOperator, AssignmentTarget, AssignmentTargetPropertyIdentifier,
    AssignmentTargetPropertyProperty, BindingPattern, BindingProperty, CallExpression, Class, ClassElement,
    ComputedMemberExpression, Declaration, ExportDefaultDeclarationKind, Expression, IdentifierReference,
    JSXMemberExpression, MethodDefinitionKind, ObjectPropertyKind, PrivateFieldExpression, PrivateInExpression,
    Program, PropertyDefinition, PropertyKey, Statement, StaticMemberExpression, TSAccessibility, TSEnumDeclaration,
    TSInterfaceDeclaration, TSLiteral, TSLiteralType, TSQualifiedName, TSSignature, TSType, TSTypeLiteral, TSTypeName,
    TSTypeOperatorOperator, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;
use oxc_span::Span as OxcSpan;
use skif_core::{HashMap, HashSet};

use crate::collect::{
    Ctx, ImportBinding, binding_ids, enum_member_name, import_bindings, member_removal, mentions_word,
};
use crate::jsdoc::leading_tags;
use crate::model::{
    ClassMember, ClassMemberKind, ClassShape, Export, ExportKind, Heritage, ImportedKeyRead, ImportedKind,
    ModuleInfo, StringConstant,
};
use crate::semantic::Analysis;

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
    type_only: bool,
    key_span: OxcSpan,
    span: OxcSpan,
}

/// Whether the field `property` exists only for the type checker: a `declare` field, or one typed `void`, `never`,
/// `undefined` or `unique symbol` whose value, if it has one, is `undefined`. A nominal brand
/// (`readonly __brand!: unique symbol`) or a phantom type parameter (`declare readonly _Options: Options`) is never
/// read at runtime, so no read can be expected of it.
fn is_type_only(property: &PropertyDefinition<'_>) -> bool {
    if property.declare {
        return true;
    }
    let phantom_type = property
        .type_annotation
        .as_ref()
        .is_some_and(|annotation| match &annotation.type_annotation {
            TSType::TSVoidKeyword(_) | TSType::TSNeverKeyword(_) | TSType::TSUndefinedKeyword(_) => true,
            TSType::TSTypeOperator(operator) => {
                operator.operator == TSTypeOperatorOperator::Unique
                    && matches!(operator.type_annotation, TSType::TSSymbolKeyword(_))
            }
            _ => false,
        });
    let no_value = property.value.as_ref().is_none_or(
        |value| matches!(value.get_inner_expression(), Expression::Identifier(id) if id.name == "undefined"),
    );
    phantom_type && no_value
}

fn declared(element: &ClassElement<'_>) -> Option<Declared> {
    let type_only = match element {
        ClassElement::PropertyDefinition(property) => is_type_only(property),
        _ => false,
    };
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
        type_only,
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
            type_only: member.type_only,
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
/// [`ModuleInfo::property_writes`], the element accesses through an imported constant in
/// [`ModuleInfo::imported_key_reads`], and the strings behind its exports in [`ModuleInfo::string_constants`].
pub(crate) fn property_names(program: &Program<'_>, analysis: &Analysis<'_>, info: &mut ModuleInfo) {
    let constants = constants(program);
    let imports = import_bindings(program);
    let mut names = PropertyNames {
        scoping: analysis.scoping(),
        constants: &constants,
        imports: &imports,
        reads: BTreeSet::new(),
        writes: BTreeSet::new(),
        imported: BTreeSet::new(),
    };
    names.visit_program(program);
    info.add_property_reads(names.reads);
    info.property_writes.extend(names.writes);
    info.property_writes.sort();
    info.property_writes.dedup();
    info.imported_key_reads.extend(names.imported);
    info.imported_key_reads.sort();
    info.imported_key_reads.dedup();
    let exported = exported_constants(&constants, &info.exports);
    info.string_constants.extend(exported);
    info.string_constants.sort();
    info.string_constants.dedup();
}

/// The strings the top-level declarations of a module bind, usable as a property key, by binding and member:
/// `const KEY = 'k'` is `(KEY, None)`, `enum E { M = 'k' }` and `const E = { M: 'k' } as const` are `(E, Some(M))`.
type Constants = HashMap<(String, Option<String>), String>;

fn constants(program: &Program<'_>) -> Constants {
    let mut out = Constants::default();
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => &e.declaration,
            other => match other.as_declaration() {
                Some(declaration) => declaration,
                None => continue,
            },
        };
        match declaration {
            Declaration::TSEnumDeclaration(declaration) => enum_constants(declaration, &mut out),
            Declaration::VariableDeclaration(variables) if variables.kind.is_const() => {
                for declarator in &variables.declarations {
                    variable_constants(declarator, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

/// The string members of an enum (`SKIP = '__v_skip'`); a numeric or computed member holds no property name.
fn enum_constants(declaration: &TSEnumDeclaration<'_>, out: &mut Constants) {
    let binding = declaration.id.name.as_str();
    for member in &declaration.body.members {
        let value = member.initializer.as_ref().and_then(literal_name);
        if let (Some(name), Some(value)) = (enum_member_name(&member.id), value) {
            out.insert((binding.to_owned(), Some(name)), value);
        }
    }
}

/// A `const` bound to a string, or the string properties of a `const` bound to an `as const` object, which no code
/// can change. A plain object's properties can be reassigned, so they are no constants.
fn variable_constants(declarator: &VariableDeclarator<'_>, out: &mut Constants) {
    let (BindingPattern::BindingIdentifier(id), Some(init)) = (&declarator.id, &declarator.init) else {
        return;
    };
    let binding = id.name.as_str();
    if let Some(value) = literal_name(init) {
        out.insert((binding.to_owned(), None), value);
        return;
    }
    let Expression::TSAsExpression(assertion) = init.without_parentheses() else {
        return;
    };
    if !assertion.type_annotation.is_const_type_reference() {
        return;
    }
    let Expression::ObjectExpression(object) = assertion.expression.get_inner_expression() else {
        return;
    };
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            continue;
        };
        if let (Some(name), Some(value)) = (key_name(&property.key), literal_name(&property.value)) {
            out.insert((binding.to_owned(), Some(name)), value);
        }
    }
}

/// The constants behind the local exports of the module, under their export names.
fn exported_constants(constants: &Constants, exports: &[Export]) -> Vec<StringConstant> {
    if constants.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for export in exports.iter().filter(|export| export.from.is_none()) {
        let Some(local) = export.local_name.as_deref() else {
            continue;
        };
        for ((binding, member), value) in constants {
            if binding == local {
                out.push(StringConstant {
                    export: export.name.clone(),
                    member: member.clone(),
                    value: value.clone(),
                });
            }
        }
    }
    out
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

/// What the key of an element access names.
enum Key {
    /// A property name: a literal, or a constant of the module.
    Name(String),
    /// A constant imported from another module, which the module graph resolves.
    Imported(ImportedKeyRead),
}

struct PropertyNames<'s> {
    /// The scopes of the program, to tell a top-level constant from a local or parameter that shadows it.
    scoping: &'s Scoping,
    constants: &'s Constants,
    imports: &'s HashMap<String, ImportBinding>,
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
    imported: BTreeSet<ImportedKeyRead>,
}

impl PropertyNames<'_> {
    fn read(&mut self, name: &str) {
        if !self.reads.contains(name) {
            self.reads.insert(name.to_owned());
        }
    }

    /// The property the element access key `expression` names: a literal, a constant (`KEY`), a member of an enum or
    /// `as const` object (`Flags.SKIP`), or one of those through a namespace import (`ns.KEY`, `ns.Flags.SKIP`).
    fn key(&self, expression: &Expression<'_>) -> Option<Key> {
        if let Some(name) = literal_name(expression) {
            return Some(Key::Name(name));
        }
        match expression.get_inner_expression() {
            Expression::Identifier(id) => self.constant(id, None),
            Expression::StaticMemberExpression(member) => self.member_constant(member),
            _ => None,
        }
    }

    /// The constant `member` names: `Flags.SKIP`, `ns.KEY` or `ns.Flags.SKIP`.
    fn member_constant(&self, member: &StaticMemberExpression<'_>) -> Option<Key> {
        let property = member.property.name.as_str();
        let inner = match member.object.get_inner_expression() {
            Expression::Identifier(id) => return self.constant(id, Some(property)),
            Expression::StaticMemberExpression(inner) => inner,
            _ => return None,
        };
        let Expression::Identifier(id) = inner.object.get_inner_expression() else {
            return None;
        };
        let binding = self.imports.get(self.top_level(id)?)?;
        matches!(binding.kind, ImportedKind::Namespace).then(|| {
            Key::Imported(ImportedKeyRead {
                specifier: binding.specifier.clone(),
                export: inner.property.name.as_str().to_owned(),
                member: Some(property.to_owned()),
            })
        })
    }

    /// The name of the top-level binding `id` refers to; `None` when it refers to a local or parameter, or to
    /// nothing the module binds.
    fn top_level<'i>(&self, id: &'i IdentifierReference<'_>) -> Option<&'i str> {
        let symbol = id
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())?;
        (self.scoping.symbol_scope_id(symbol) == self.scoping.root_scope_id()).then_some(id.name.as_str())
    }

    /// The constant `id`, or its `member`, holds: a constant of the module, or one imported.
    fn constant(&self, id: &IdentifierReference<'_>, member: Option<&str>) -> Option<Key> {
        let name = self.top_level(id)?;
        let Some(binding) = self.imports.get(name) else {
            let key = (name.to_owned(), member.map(str::to_owned));
            return self.constants.get(&key).cloned().map(Key::Name);
        };
        let (export, member) = match &binding.kind {
            ImportedKind::Named(export) => (export.clone(), member),
            ImportedKind::Default => ("default".to_owned(), member),
            ImportedKind::Namespace => (member?.to_owned(), None),
        };
        Some(Key::Imported(ImportedKeyRead {
            specifier: binding.specifier.clone(),
            export,
            member: member.map(str::to_owned),
        }))
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

impl<'a> Visit<'a> for PropertyNames<'_> {
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if it.operator == AssignmentOperator::Assign {
            let written = match &it.left {
                AssignmentTarget::StaticMemberExpression(member) => {
                    Some((member.property.name.as_str().to_owned(), &member.object))
                }
                AssignmentTarget::ComputedMemberExpression(member) => match self.key(&member.expression) {
                    Some(Key::Name(name)) => Some((name, &member.object)),
                    Some(Key::Imported(_)) | None => None,
                },
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
        match self.key(&it.expression) {
            Some(Key::Name(name)) => self.read(&name),
            Some(Key::Imported(read)) => {
                self.imported.insert(read);
            }
            None => {}
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
