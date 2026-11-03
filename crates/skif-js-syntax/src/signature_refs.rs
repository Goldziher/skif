//! Which exports mention other exports in their signature.
//!
//! An exported type that only appears in the signature of another exported value (`export function f(p: Props)`)
//! is part of that value's public surface: it is used exactly when the value is. Type-level containers
//! (`interface`, `type`) are not followed, matching how knip treats `ignoreExportsUsedInFile` by default.

use std::collections::{HashMap, HashSet};

use oxc_ast::ast::{
    ArrowFunctionBody, Class, Declaration, ExportDefaultDeclarationKind, Expression, Function, FunctionBody, Program,
    Statement, TSTypeName,
};
use oxc_ast_visit::{Visit, walk};

use crate::model::{ExportKind, ModuleInfo};

/// Type names mentioned in one declaration, optionally leaving out function bodies.
struct TypeNames {
    names: HashSet<String>,
    signature_only: bool,
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
    let mut visitor = TypeNames {
        names: HashSet::new(),
        signature_only,
    };
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
    let mut visitor = TypeNames {
        names: HashSet::new(),
        signature_only: true,
    };
    visitor.visit_class(class);
    visitor.names
}

fn expression_names(expression: &Expression<'_>) -> HashSet<String> {
    let explicit_return = match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => arrow.return_type.is_some(),
        Expression::FunctionExpression(function) => function.return_type.is_some(),
        _ => false,
    };
    let mut visitor = TypeNames {
        names: HashSet::new(),
        signature_only: explicit_return,
    };
    visitor.visit_expression(expression);
    visitor.names
}

/// The value-level declarations of `declaration` with the type names their signatures mention.
fn containers(declaration: &Declaration<'_>, out: &mut Vec<(String, HashSet<String>)>) {
    match declaration {
        Declaration::FunctionDeclaration(function) => {
            if let Some(id) = &function.id {
                out.push((
                    id.name.as_str().to_owned(),
                    function_names(function, function.return_type.is_some()),
                ));
            }
        }
        Declaration::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                out.push((id.name.as_str().to_owned(), class_names(class)));
            }
        }
        Declaration::VariableDeclaration(variables) => {
            for declarator in &variables.declarations {
                let Some(id) = declarator.id.get_binding_identifier() else {
                    continue;
                };
                let names = if let Some(annotation) = &declarator.type_annotation {
                    let mut visitor = TypeNames {
                        names: HashSet::new(),
                        signature_only: true,
                    };
                    visitor.visit_ts_type_annotation(annotation);
                    visitor.names
                } else if let Some(init) = &declarator.init {
                    expression_names(init)
                } else {
                    continue;
                };
                out.push((id.name.as_str().to_owned(), names));
            }
        }
        _ => {}
    }
}

/// Fills [`crate::model::Export::referenced_in`] for every export mentioned in the signature of another value export.
pub(crate) fn mark(program: &Program<'_>, info: &mut ModuleInfo) {
    let mut found: Vec<(String, HashSet<String>)> = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => containers(&export.declaration, &mut found),
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                    if let Some(id) = &function.id {
                        found.push((
                            id.name.as_str().to_owned(),
                            function_names(function, function.return_type.is_some()),
                        ));
                    }
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    if let Some(id) = &class.id {
                        found.push((id.name.as_str().to_owned(), class_names(class)));
                    }
                }
                _ => {}
            },
            other => {
                if let Some(declaration) = other.as_declaration() {
                    containers(declaration, &mut found);
                }
            }
        }
    }
    let mut mentions: HashMap<&str, &HashSet<String>> = HashMap::new();
    for (local, names) in &found {
        mentions.insert(local.as_str(), names);
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
    let mut referenced_in: Vec<Vec<String>> = Vec::with_capacity(info.exports.len());
    for export in &info.exports {
        let mut containing = Vec::new();
        if let Some(local) = export.local_name.as_deref() {
            for (container, names) in &mentions {
                if *container != local && names.contains(local) {
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
