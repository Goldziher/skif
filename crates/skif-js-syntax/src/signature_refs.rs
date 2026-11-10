//! Which exports mention other exports in their signature.
//!
//! An exported type that only appears in the signature of another exported value (`export function f(p: Props)`)
//! is part of that value's public surface: it is used exactly when the value is. Type-level containers
//! (`interface`, `type`) are not followed, matching how knip treats `ignoreExportsUsedInFile` by default, unless a value
//! export shares the name (`export const Service = …; export type Service = …`, `export const IStore =
//! createDecorator<IStore>(…); export interface IStore { … }`): knip keeps one export per name, a value, so what the type
//! mentions is used whenever the name is, as a type or as a value.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use oxc_ast::ast::{
    ArrowFunctionBody, Class, Declaration, ExportDefaultDeclarationKind, Expression, Function, FunctionBody, Program,
    Statement, TSTypeName,
};
use oxc_ast_visit::{Visit, walk};

use crate::model::{Export, ExportKind, ImportedKind, ModuleInfo};

/// A declaration's local name with the type names it mentions, shared by the bindings of one destructuring declarator.
type Container = (String, Rc<HashSet<String>>);

/// Type names mentioned in one declaration, optionally leaving out function bodies.
struct TypeNames {
    names: HashSet<String>,
    signature_only: bool,
}

impl TypeNames {
    fn new(signature_only: bool) -> Self {
        Self {
            names: HashSet::new(),
            signature_only,
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

    fn visit_function_body(&mut self, it: &FunctionBody<'a>) {
        if !self.signature_only {
            walk::walk_function_body(self, it);
        }
    }

    fn visit_arrow_function_body(&mut self, it: &ArrowFunctionBody<'a>) {
        if !self.signature_only {
            walk::walk_arrow_function_body(self, it);
        }
    }
}

fn function_names(function: &Function<'_>, signature_only: bool) -> HashSet<String> {
    let mut visitor = TypeNames::new(signature_only);
    if let Some(parameters) = &function.type_parameters {
        visitor.visit_ts_type_parameter_declaration(parameters);
    }
    visitor.visit_formal_parameters(&function.params);
    if let Some(return_type) = &function.return_type {
        visitor.visit_ts_type_annotation(return_type);
    }
    if let Some(body) = &function.body {
        visitor.visit_function_body(body);
    }
    visitor.names
}

fn class_names(class: &Class<'_>) -> HashSet<String> {
    let mut visitor = TypeNames::new(true);
    visitor.visit_class(class);
    visitor.names
}

fn expression_names(expression: &Expression<'_>) -> HashSet<String> {
    let explicit_return = match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => arrow.return_type.is_some(),
        Expression::FunctionExpression(function) => function.return_type.is_some(),
        _ => false,
    };
    let mut visitor = TypeNames::new(explicit_return);
    visitor.visit_expression(expression);
    visitor.names
}

/// The value-level declarations of `declaration` with the type names their signatures mention.
fn containers(declaration: &Declaration<'_>, out: &mut Vec<Container>) {
    match declaration {
        Declaration::FunctionDeclaration(function) => {
            if let Some(id) = &function.id {
                out.push((
                    id.name.as_str().to_owned(),
                    Rc::new(function_names(function, function.return_type.is_some())),
                ));
            }
        }
        Declaration::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                out.push((id.name.as_str().to_owned(), Rc::new(class_names(class))));
            }
        }
        Declaration::VariableDeclaration(variables) => {
            for declarator in &variables.declarations {
                let ids = declarator.id.get_binding_identifiers();
                if ids.is_empty() {
                    continue;
                }
                let names = Rc::new(if let Some(annotation) = &declarator.type_annotation {
                    let mut visitor = TypeNames::new(true);
                    visitor.visit_ts_type_annotation(annotation);
                    visitor.names
                } else if let Some(init) = &declarator.init {
                    expression_names(init)
                } else {
                    continue;
                });
                out.extend(ids.iter().map(|id| (id.name.as_str().to_owned(), Rc::clone(&names))));
            }
        }
        _ => {}
    }
}

/// The type-level declaration `declaration` (an `interface` or a `type` alias) with the type names it mentions,
/// including the interfaces it extends. It contains them only for a value export of its name, which can be a separate
/// clause (`type X = …; const X = …; export { X }`).
fn type_container(declaration: &Declaration<'_>, out: &mut Vec<Container>) {
    let mut visitor = TypeNames::new(true);
    let name = match declaration {
        Declaration::TSTypeAliasDeclaration(alias) => {
            visitor.visit_ts_type_alias_declaration(alias);
            &alias.id.name
        }
        Declaration::TSInterfaceDeclaration(interface) => {
            visitor.visit_ts_interface_declaration(interface);
            &interface.id.name
        }
        _ => return,
    };
    out.push((name.as_str().to_owned(), Rc::new(visitor.names)));
}

/// The local bindings of the imports by specifier and imported name. A name imported twice keeps its first binding.
fn import_bindings(info: &ModuleInfo) -> HashMap<(&str, &ImportedKind), &str> {
    let mut bindings = HashMap::new();
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

/// The declarations of `program` that can contain type names, exported or not, with the names they mention.
fn collect_containers(program: &Program<'_>) -> Vec<Container> {
    let mut found = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => {
                containers(&export.declaration, &mut found);
                type_container(&export.declaration, &mut found);
            }
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                    if let Some(id) = &function.id {
                        found.push((
                            id.name.as_str().to_owned(),
                            Rc::new(function_names(function, function.return_type.is_some())),
                        ));
                    }
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    if let Some(id) = &class.id {
                        found.push((id.name.as_str().to_owned(), Rc::new(class_names(class))));
                    }
                }
                _ => {}
            },
            other => {
                if let Some(declaration) = other.as_declaration() {
                    containers(declaration, &mut found);
                    type_container(declaration, &mut found);
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
    let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
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
    let found = collect_containers(program);
    // One name can have several declarations (a value and a type, overloads): it mentions what any of them does.
    let mut mentions: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (local, names) in &found {
        mentions
            .entry(local.as_str())
            .or_default()
            .extend(names.iter().map(String::as_str));
    }
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
        .fold(HashMap::new(), |mut map, (local, name)| {
            map.entry(local).or_default().push(name);
            map
        });
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
