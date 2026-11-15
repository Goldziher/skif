//! Which exports mention other exports in their signature.
//!
//! An exported type that only appears in the signature of another exported value (`export function f(p: Props)`)
//! is part of that value's public surface: it is used exactly when the value is. Type-level containers
//! (`interface`, `type`) are not followed, matching how knip treats `ignoreExportsUsedInFile` by default, unless a value
//! export shares the name (`export const Service = …; export type Service = …`, `export const IStore =
//! createDecorator<IStore>(…); export interface IStore { … }`): knip keeps one export per name, a value, so what the type
//! mentions is used whenever the name is, as a type or as a value.
//!
//! A signature left to inference takes types from the top-level declarations it calls or passes on
//! (`export const useCtx = () => useContext(Context)` has the type `Context` was created with), so those are followed:
//! the result of a called function (its return type, else its signature), of a method called on an object literal, and
//! the signature of an identifier passed to a call outside a function body. An explicit return type ends the walk.

use skif_core::{HashMap, HashSet};
use std::rc::Rc;

use oxc_ast::ast::{
    Argument, ArrowFunctionBody, ArrowFunctionExpression, BindingPattern, CallExpression, Class, Declaration,
    ExportDefaultDeclarationKind, Expression, Function, FunctionBody, ObjectPropertyKind, Program, PropertyKey,
    Statement, TSType, TSTypeName, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};

use crate::model::{Export, ExportKind, ImportedKind, ModuleInfo};

/// A declaration's local name with the type names it mentions, shared by the bindings of one destructuring declarator.
type Container = (String, Rc<HashSet<String>>);

/// A top-level declaration whose type can flow into an inferred signature, named in it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Follow {
    /// `f(…)`: the result of calling the declaration `f`.
    Result(String),
    /// `object.method(…)`: the result of calling `method` of the object literal `object`.
    Member(String, String),
    /// `g(x)` outside a function body: the signature of the declaration `x`.
    Declaration(String),
}

/// Type names mentioned in one declaration, optionally leaving out function bodies, with the declarations it calls or
/// passes on.
struct TypeNames {
    names: HashSet<String>,
    signature_only: bool,
    follows: Vec<Follow>,
    /// How many function bodies the walk is in: an argument passed in a body does not reach the signature.
    body_depth: usize,
    /// Whether each arrow function the walk is in has a return type, innermost last.
    typed_arrows: Vec<bool>,
}

impl TypeNames {
    fn new(signature_only: bool) -> Self {
        Self {
            names: HashSet::default(),
            signature_only,
            follows: Vec::new(),
            body_depth: 0,
            typed_arrows: Vec::new(),
        }
    }
}

impl<'a> Visit<'a> for TypeNames {
    fn visit_ts_type_name(&mut self, it: &TSTypeName<'a>) {
        if let TSTypeName::IdentifierReference(id) = it {
            self.names.insert(id.name.as_str().to_owned());
        }
        walk::walk_ts_type_name(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        match &it.callee {
            Expression::Identifier(callee) => self.follows.push(Follow::Result(callee.name.as_str().to_owned())),
            Expression::StaticMemberExpression(member) => {
                if let Expression::Identifier(object) = &member.object {
                    self.follows.push(Follow::Member(
                        object.name.as_str().to_owned(),
                        member.property.name.as_str().to_owned(),
                    ));
                }
            }
            _ => {}
        }
        if self.body_depth == 0 {
            for argument in &it.arguments {
                if let Argument::Identifier(id) = argument {
                    self.follows.push(Follow::Declaration(id.name.as_str().to_owned()));
                }
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_function_body(&mut self, it: &FunctionBody<'a>) {
        if !self.signature_only {
            self.body_depth += 1;
            walk::walk_function_body(self, it);
            self.body_depth -= 1;
        }
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.typed_arrows.push(it.return_type.is_some());
        walk::walk_arrow_function_expression(self, it);
        self.typed_arrows.pop();
    }

    /// An expression body without a return type is the arrow's result, so it is part of the signature.
    fn visit_arrow_function_body(&mut self, it: &ArrowFunctionBody<'a>) {
        let inferred = !matches!(it, ArrowFunctionBody::FunctionBody(_)) && self.typed_arrows.last() == Some(&false);
        if !self.signature_only || inferred {
            walk::walk_arrow_function_body(self, it);
        }
    }
}

/// A top-level declaration by its local name, as [`Follow`] looks it up.
#[derive(Clone, Copy)]
enum Local<'p, 'a> {
    Function(&'p Function<'a>),
    Class(&'p Class<'a>),
    Variable(&'p VariableDeclarator<'a>),
}

/// The top-level function, class and single-binding variable declarations of `program`, exported or not. A later
/// declaration of a name replaces an earlier one; a function without a body (an overload or `declare`) is left out.
fn locals<'p, 'a>(program: &'p Program<'a>) -> HashMap<&'p str, Local<'p, 'a>> {
    let mut locals = HashMap::default();
    let mut add = |declaration: &'p Declaration<'a>| match declaration {
        Declaration::FunctionDeclaration(function) => {
            if let (Some(id), Some(_)) = (&function.id, &function.body) {
                locals.insert(id.name.as_str(), Local::Function(function));
            }
        }
        Declaration::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                locals.insert(id.name.as_str(), Local::Class(class));
            }
        }
        Declaration::VariableDeclaration(variables) => {
            for declarator in &variables.declarations {
                if let BindingPattern::BindingIdentifier(id) = &declarator.id {
                    locals.insert(id.name.as_str(), Local::Variable(declarator));
                }
            }
        }
        _ => {}
    };
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => add(&export.declaration),
            other => {
                if let Some(declaration) = other.as_declaration() {
                    add(declaration);
                }
            }
        }
    }
    locals
}

/// Walks the signature of `function`: type parameters, parameters, return type, and no body.
fn visit_function_signature(visitor: &mut TypeNames, function: &Function<'_>) {
    if let Some(parameters) = &function.type_parameters {
        visitor.visit_ts_type_parameter_declaration(parameters);
    }
    visitor.visit_formal_parameters(&function.params);
    if let Some(return_type) = &function.return_type {
        visitor.visit_ts_type_annotation(return_type);
    }
}

/// Walks what calling a function yields: its type parameters and return type when it has one, else its signature,
/// with an arrow's expression body.
fn visit_function_result(visitor: &mut TypeNames, function: &Function<'_>) {
    if let Some(return_type) = &function.return_type {
        if let Some(parameters) = &function.type_parameters {
            visitor.visit_ts_type_parameter_declaration(parameters);
        }
        visitor.visit_ts_type_annotation(return_type);
    } else {
        visit_function_signature(visitor, function);
    }
}

fn visit_arrow_result(visitor: &mut TypeNames, arrow: &ArrowFunctionExpression<'_>) {
    if let Some(return_type) = &arrow.return_type {
        if let Some(parameters) = &arrow.type_parameters {
            visitor.visit_ts_type_parameter_declaration(parameters);
        }
        visitor.visit_ts_type_annotation(return_type);
    } else {
        visitor.visit_arrow_function_expression(arrow);
    }
}

/// Walks the result of calling the value `expression`, when it is a function.
fn visit_value_result(visitor: &mut TypeNames, expression: &Expression<'_>) -> bool {
    match expression {
        Expression::ArrowFunctionExpression(arrow) => visit_arrow_result(visitor, arrow),
        Expression::FunctionExpression(function) => visit_function_result(visitor, function),
        _ => return false,
    }
    true
}

fn visit_call_result(visitor: &mut TypeNames, local: Local<'_, '_>) {
    match local {
        Local::Function(function) => visit_function_result(visitor, function),
        Local::Class(class) => visitor.visit_class(class),
        Local::Variable(declarator) => {
            if let Some(annotation) = &declarator.type_annotation {
                if let TSType::TSFunctionType(function) = &annotation.type_annotation {
                    if let Some(parameters) = &function.type_parameters {
                        visitor.visit_ts_type_parameter_declaration(parameters);
                    }
                    visitor.visit_ts_type_annotation(&function.return_type);
                } else {
                    visitor.visit_ts_type_annotation(annotation);
                }
            } else if !declarator
                .init
                .as_ref()
                .is_some_and(|init| visit_value_result(visitor, init))
            {
                visitor.visit_variable_declarator(declarator);
            }
        }
    }
}

fn visit_member_result(visitor: &mut TypeNames, local: Local<'_, '_>, method: &str) {
    let Local::Variable(VariableDeclarator {
        init: Some(Expression::ObjectExpression(object)),
        ..
    }) = local
    else {
        return;
    };
    let value = object.properties.iter().find_map(|property| match property {
        ObjectPropertyKind::ObjectProperty(property)
            if !property.computed
                && matches!(&property.key, PropertyKey::StaticIdentifier(key) if key.name.as_str() == method) =>
        {
            Some(&property.value)
        }
        _ => None,
    });
    if let Some(value) = value {
        visit_value_result(visitor, value);
    }
}

fn visit_declaration(visitor: &mut TypeNames, local: Local<'_, '_>) {
    match local {
        Local::Function(function) => visit_function_signature(visitor, function),
        Local::Class(class) => visitor.visit_class(class),
        Local::Variable(declarator) => visitor.visit_variable_declarator(declarator),
    }
}

/// The type names `visitor` found, with those of the declarations it calls or passes on, followed transitively. A
/// followed declaration is read as a signature, without function bodies.
fn with_followed(mut visitor: TypeNames, locals: &HashMap<&str, Local<'_, '_>>) -> HashSet<String> {
    let mut seen: HashSet<Follow> = HashSet::default();
    visitor.signature_only = true;
    while let Some(follow) = visitor.follows.pop() {
        let name = match &follow {
            Follow::Result(name) | Follow::Member(name, _) | Follow::Declaration(name) => name.as_str(),
        };
        let Some(local) = locals.get(name).copied() else {
            continue;
        };
        if !seen.insert(follow.clone()) {
            continue;
        }
        visitor.body_depth = 0;
        match &follow {
            Follow::Result(_) => visit_call_result(&mut visitor, local),
            Follow::Member(_, method) => visit_member_result(&mut visitor, local, method),
            Follow::Declaration(_) => visit_declaration(&mut visitor, local),
        }
    }
    visitor.names
}

fn function_names(function: &Function<'_>, signature_only: bool, locals: &Locals<'_, '_>) -> HashSet<String> {
    let mut visitor = TypeNames::new(signature_only);
    visit_function_signature(&mut visitor, function);
    if let Some(body) = &function.body {
        visitor.visit_function_body(body);
    }
    with_followed(visitor, locals)
}

fn class_names(class: &Class<'_>, locals: &Locals<'_, '_>) -> HashSet<String> {
    let mut visitor = TypeNames::new(true);
    visitor.visit_class(class);
    with_followed(visitor, locals)
}

fn expression_names(expression: &Expression<'_>, locals: &Locals<'_, '_>) -> HashSet<String> {
    let explicit_return = match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => arrow.return_type.is_some(),
        Expression::FunctionExpression(function) => function.return_type.is_some(),
        _ => false,
    };
    let mut visitor = TypeNames::new(explicit_return);
    visitor.visit_expression(expression);
    // An explicit return type is the whole result: nothing the body calls or passes on reaches the signature.
    if explicit_return {
        return visitor.names;
    }
    with_followed(visitor, locals)
}

/// Which declarations to collect, by local name.
type Wanted<'w> = &'w dyn Fn(&str) -> bool;

/// The top-level declarations of a program by local name ([`locals`]).
type Locals<'p, 'a> = HashMap<&'p str, Local<'p, 'a>>;

/// The value-level declarations of `declaration` named in `wanted` with the type names their signatures mention.
fn containers(declaration: &Declaration<'_>, wanted: Wanted<'_>, locals: &Locals<'_, '_>, out: &mut Vec<Container>) {
    match declaration {
        Declaration::FunctionDeclaration(function) => {
            if let Some(id) = &function.id
                && wanted(id.name.as_str())
            {
                out.push((
                    id.name.as_str().to_owned(),
                    Rc::new(function_names(function, function.return_type.is_some(), locals)),
                ));
            }
        }
        Declaration::ClassDeclaration(class) => {
            if let Some(id) = &class.id
                && wanted(id.name.as_str())
            {
                out.push((id.name.as_str().to_owned(), Rc::new(class_names(class, locals))));
            }
        }
        Declaration::VariableDeclaration(variables) => {
            for declarator in &variables.declarations {
                let ids = declarator.id.get_binding_identifiers();
                if !ids.iter().any(|id| wanted(id.name.as_str())) {
                    continue;
                }
                let names = Rc::new(if let Some(annotation) = &declarator.type_annotation {
                    let mut visitor = TypeNames::new(true);
                    visitor.visit_ts_type_annotation(annotation);
                    visitor.names
                } else if let Some(init) = &declarator.init {
                    expression_names(init, locals)
                } else {
                    continue;
                });
                out.extend(ids.iter().map(|id| (id.name.as_str().to_owned(), Rc::clone(&names))));
            }
        }
        _ => {}
    }
}

/// The type-level declaration `declaration` (an `interface` or a `type` alias), when `wanted` names it, with the type
/// names it mentions, including the interfaces it extends. It contains them only for a value export of its name, which
/// can be a separate clause (`type X = …; const X = …; export { X }`).
fn type_container(declaration: &Declaration<'_>, wanted: Wanted<'_>, out: &mut Vec<Container>) {
    let mut visitor = TypeNames::new(true);
    let name = match declaration {
        Declaration::TSTypeAliasDeclaration(alias) if wanted(alias.id.name.as_str()) => {
            visitor.visit_ts_type_alias_declaration(alias);
            &alias.id.name
        }
        Declaration::TSInterfaceDeclaration(interface) if wanted(interface.id.name.as_str()) => {
            visitor.visit_ts_interface_declaration(interface);
            &interface.id.name
        }
        _ => return,
    };
    out.push((name.as_str().to_owned(), Rc::new(visitor.names)));
}

/// The local bindings of the imports by specifier and imported name. A name imported twice keeps its first binding.
fn import_bindings(info: &ModuleInfo) -> HashMap<(&str, &ImportedKind), &str> {
    let mut bindings = HashMap::default();
    for import in &info.imports {
        for name in &import.names {
            if let Some(local) = name.local.as_deref() {
                bindings.entry((import.specifier.as_str(), &name.kind)).or_insert(local);
            }
        }
    }
    bindings
}

/// The local binding of an import that brings in what `export { a } from "x"` re-exports: `a` of `import { a } from
/// "x"` (same specifier, same name). A signature that mentions the binding mentions the re-export, as in knip, which
/// matches the names in a signature against the exported names.
fn reexported_binding<'a>(bindings: &HashMap<(&str, &ImportedKind), &'a str>, export: &Export) -> Option<&'a str> {
    if export.kind != ExportKind::ReExport || export.local_name.is_some() {
        return None;
    }
    let from = export.from.as_deref()?;
    let wanted = match export.imported_name.as_deref()? {
        "default" => ImportedKind::Default,
        name => ImportedKind::Named(name.to_owned()),
    };
    bindings.get(&(from, &wanted)).copied()
}

/// The declarations of `program` named in `wanted` that can contain type names, exported or not, with the names they
/// mention.
fn collect_containers(program: &Program<'_>, wanted: Wanted<'_>) -> Vec<Container> {
    let locals = locals(program);
    let mut found = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => {
                containers(&export.declaration, wanted, &locals, &mut found);
                type_container(&export.declaration, wanted, &mut found);
            }
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                    if let Some(id) = &function.id
                        && wanted(id.name.as_str())
                    {
                        found.push((
                            id.name.as_str().to_owned(),
                            Rc::new(function_names(function, function.return_type.is_some(), &locals)),
                        ));
                    }
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    if let Some(id) = &class.id
                        && wanted(id.name.as_str())
                    {
                        found.push((id.name.as_str().to_owned(), Rc::new(class_names(class, &locals))));
                    }
                }
                _ => {}
            },
            other => {
                if let Some(declaration) = other.as_declaration() {
                    containers(declaration, wanted, &locals, &mut found);
                    type_container(declaration, wanted, &mut found);
                }
            }
        }
    }
    found
}

/// The containers that mention each name, by name. Only value exports can contain: the other containers are left out.
fn mentioned_by<'a>(
    mentions: &HashMap<&'a str, HashSet<&'a str>>,
    exported_as: &HashMap<&str, Vec<String>>,
) -> HashMap<&'a str, Vec<&'a str>> {
    let mut by_name: HashMap<&str, Vec<&str>> = HashMap::default();
    for (container, names) in mentions {
        if !exported_as.contains_key(container) {
            continue;
        }
        for name in names {
            by_name.entry(*name).or_default().push(*container);
        }
    }
    by_name
}

/// Fills [`crate::model::Export::referenced_in`] for every export mentioned in the signature of another value export.
pub(crate) fn mark(program: &Program<'_>, info: &mut ModuleInfo) {
    let exported_as: HashMap<&str, Vec<String>> = info
        .exports
        .iter()
        .filter(|export| {
            !export.is_type
                && !matches!(
                    export.kind,
                    ExportKind::ReExport | ExportKind::StarReExport | ExportKind::Type
                )
        })
        .filter_map(|export| Some((export.local_name.as_deref()?, export.name.clone())))
        .fold(HashMap::default(), |mut map, (local, name)| {
            map.entry(local).or_default().push(name);
            map
        });
    // Only a value export contains: without one, no export is referenced in a signature, and the declarations of other
    // names are not read at all.
    if exported_as.is_empty() {
        return;
    }
    let found = collect_containers(program, &|name: &str| exported_as.contains_key(name));
    // One name can have several declarations (a value and a type, overloads): it mentions what any of them does.
    let mut mentions: HashMap<&str, HashSet<&str>> = HashMap::default();
    for (local, names) in &found {
        mentions
            .entry(local.as_str())
            .or_default()
            .extend(names.iter().map(String::as_str));
    }
    let mentioned_by = mentioned_by(&mentions, &exported_as);
    let bindings = import_bindings(info);
    let mut referenced_in: Vec<Vec<String>> = Vec::with_capacity(info.exports.len());
    for export in &info.exports {
        let mut containing = Vec::new();
        if let Some(local) = export
            .local_name
            .as_deref()
            .or_else(|| reexported_binding(&bindings, export))
        {
            for container in mentioned_by.get(local).into_iter().flatten() {
                if *container != local {
                    containing.extend(exported_as.get(container).into_iter().flatten().cloned());
                }
            }
        }
        containing.sort();
        containing.dedup();
        referenced_in.push(containing);
    }
    for (export, containing) in info.exports.iter_mut().zip(referenced_in) {
        export.referenced_in = containing;
    }
}
