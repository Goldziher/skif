//! Static evaluation of the value a tool configuration file exports. Nothing is executed.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BinaryOperator, BindingPattern, Declaration, ExportDefaultDeclarationKind,
    Expression, ImportDeclarationSpecifier, NewExpression, ObjectPropertyKind, Program, Statement,
    StaticMemberExpression, TemplateLiteral, UnaryOperator, VariableDeclarationKind,
};
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::SourceType;
use serde::{Deserialize, Serialize};

use crate::finder::static_string;

const MAX_DEPTH: u32 = 32;

/// A statically known value from a config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigValue {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A numeric literal.
    Number(f64),
    /// A string literal or a template literal without expressions.
    String(String),
    /// An array literal; spreads of static arrays are flattened, anything else becomes [`ConfigValue::Unknown`].
    Array(Vec<ConfigValue>),
    /// An object literal in source order. A spread of an unknown value is recorded as the key `...`.
    Object(Vec<(String, ConfigValue)>),
    /// Anything that cannot be known without running the code.
    Unknown,
}

impl ConfigValue {
    /// The value of `key` when this is an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        match self {
            Self::Object(entries) => entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string when this is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The elements when this is an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[ConfigValue]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The entries when this is an object.
    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, ConfigValue)]> {
        match self {
            Self::Object(entries) => Some(entries),
            _ => None,
        }
    }

    /// The boolean when this is a boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Whether the value, or anything inside it, is [`ConfigValue::Unknown`].
    #[must_use]
    pub fn has_unknown(&self) -> bool {
        match self {
            Self::Unknown => true,
            Self::Array(items) => items.iter().any(Self::has_unknown),
            Self::Object(entries) => entries.iter().any(|(_, v)| v.has_unknown()),
            _ => false,
        }
    }
}

/// Statically evaluates what `source` exports: `export default …`, `module.exports = …` or `export = …`.
///
/// Understood: object, array, string, number, boolean and `null` literals; template literals without expressions;
/// `as`/`satisfies`/`!`/parenthesised wrappers; calls named `define*` (such as `defineConfig`), which evaluate to
/// their first argument; arrow and function expressions that return a single expression; spreads of static values; and
/// identifiers that name a top-level `const` initialised with something static; and paths computed from the config
/// file's own location (`__dirname`, `import.meta.dirname`, `new URL('./x', import.meta.url)`, `path.resolve`,
/// `path.join`, `path.dirname` and `fileURLToPath`, however `path` was imported), which evaluate to strings relative to
/// the config file's directory (`"."`, `"./src"`, `"../shared"`). Everything else is
/// [`ConfigValue::Unknown`]. Returns `None` when the file does not parse or exports nothing recognisable.
#[must_use]
pub fn extract_config_object(source: &str, path: &Path) -> Option<ConfigValue> {
    let source_type = SourceType::from_path(path).ok()?;
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return None;
    }
    let program = &ret.program;
    let scope = top_level_consts(program);
    let bindings = module_bindings(program);
    let file = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let eval = Evaluator {
        consts: &scope,
        bindings: &bindings,
        file,
    };
    exported_expression(program).map(|expr| eval.expr(expr, 0))
}

fn top_level_consts<'p, 'a>(program: &'p Program<'a>) -> HashMap<String, &'p Expression<'a>> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(Declaration::VariableDeclaration(v)) = decl
            && v.kind == VariableDeclarationKind::Const
        {
            for d in &v.declarations {
                if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init) {
                    map.insert(id.name.as_str().to_string(), init);
                }
            }
        }
    }
    map
}

/// Functions of `path`, whose results are computed lexically.
const PATH_FUNCTIONS: &[&str] = &["resolve", "join", "dirname", "normalize"];

/// The local names the config file gives to the `path` and `url` modules and their functions.
#[derive(Default)]
struct ModuleBindings {
    path_modules: HashSet<String>,
    path_functions: HashMap<String, String>,
    url_modules: HashSet<String>,
    file_url_to_path: HashSet<String>,
}

impl ModuleBindings {
    /// Records `local` as the module itself (`member` is `None`) or as its export `member`.
    fn bind(&mut self, module: &str, member: Option<&str>, local: &str) {
        let local = local.to_owned();
        match (module.strip_prefix("node:").unwrap_or(module), member) {
            ("path" | "path/posix", None) => {
                self.path_modules.insert(local);
            }
            ("path" | "path/posix", Some(name)) if PATH_FUNCTIONS.contains(&name) => {
                self.path_functions.insert(local, name.to_owned());
            }
            ("url", None) => {
                self.url_modules.insert(local);
            }
            ("url", Some("fileURLToPath")) => {
                self.file_url_to_path.insert(local);
            }
            _ => {}
        }
    }
}

fn module_bindings(program: &Program<'_>) -> ModuleBindings {
    let mut bindings = ModuleBindings::default();
    for stmt in &program.body {
        if let Statement::ImportDeclaration(d) = stmt {
            let module = d.source.value.as_str().unwrap_or_default();
            for specifier in d.specifiers.iter().flatten() {
                match specifier {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => {
                        let imported = crate::collect::module_export_name(&s.imported);
                        bindings.bind(module, Some(&imported), s.local.name.as_str());
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                        bindings.bind(module, None, s.local.name.as_str());
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        bindings.bind(module, None, s.local.name.as_str());
                    }
                }
            }
        } else if let Some(Declaration::VariableDeclaration(v)) = stmt.as_declaration() {
            for declarator in &v.declarations {
                if let Some(module) = declarator.init.as_ref().and_then(required_module) {
                    bind_required(&mut bindings, &module, &declarator.id);
                } else if let Some(Expression::Identifier(source)) =
                    declarator.init.as_ref().map(Expression::get_inner_expression)
                {
                    // `const { resolve } = path;` destructures a module binding made earlier.
                    let module = if bindings.path_modules.contains(source.name.as_str()) {
                        Some("path")
                    } else if bindings.url_modules.contains(source.name.as_str()) {
                        Some("url")
                    } else {
                        None
                    };
                    if let Some(module) = module {
                        bind_required(&mut bindings, module, &declarator.id);
                    }
                }
            }
        }
    }
    bindings
}

/// The module a `require("module")` call loads.
fn required_module(init: &Expression<'_>) -> Option<String> {
    let Expression::CallExpression(call) = init.get_inner_expression() else {
        return None;
    };
    match &call.callee {
        Expression::Identifier(id) if id.name.as_str() == "require" => call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .and_then(static_string),
        _ => None,
    }
}

fn bind_required(bindings: &mut ModuleBindings, module: &str, id: &BindingPattern<'_>) {
    match id {
        BindingPattern::BindingIdentifier(local) => bindings.bind(module, None, local.name.as_str()),
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                if let (Some(key), BindingPattern::BindingIdentifier(local)) =
                    (property.key.static_name(), &property.value)
                {
                    bindings.bind(module, Some(&crate::js_string(key.as_js_str())), local.name.as_str());
                }
            }
        }
        _ => {}
    }
}

/// Joins path segments lexically into a path relative to the config directory: `"."`, `"./a/b"` or `"../a"`.
fn normalized(parts: &[&str]) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for part in parts.iter().flat_map(|part| part.split(['/', '\\'])) {
        match part {
            "" | "." => {}
            ".." if segments.last().is_some_and(|last| *last != "..") => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    match segments.first() {
        None => ".".to_owned(),
        Some(&"..") => segments.join("/"),
        Some(_) => format!("./{}", segments.join("/")),
    }
}

/// The result of a `path` or `url` function, `Unknown` when an argument is not a known relative path.
fn apply_path_function(function: &str, args: &[ConfigValue]) -> ConfigValue {
    let Some(parts) = args.iter().map(ConfigValue::as_str).collect::<Option<Vec<&str>>>() else {
        return ConfigValue::Unknown;
    };
    let absolute = |part: &&str| part.starts_with(['/', '\\']);
    let path = match (function, parts.as_slice()) {
        ("fileURLToPath", [file]) => (*file).to_owned(),
        ("dirname", [path]) => normalized(&[*path, ".."]),
        ("normalize", [path]) => normalized(&[*path]),
        ("resolve", parts) if !parts.iter().any(absolute) => normalized(parts),
        ("join", parts) if !parts.first().is_some_and(absolute) => normalized(parts),
        _ => return ConfigValue::Unknown,
    };
    ConfigValue::String(path)
}

fn exported_expression<'p, 'a>(program: &'p Program<'a>) -> Option<&'p Expression<'a>> {
    for stmt in &program.body {
        match stmt {
            Statement::ExportDefaultDeclaration(e) => {
                return match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(_)
                    | ExportDefaultDeclarationKind::ClassDeclaration(_)
                    | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => None,
                    other => other.as_expression(),
                };
            }
            Statement::TSExportAssignment(e) => return Some(&e.expression),
            Statement::ExpressionStatement(s) => {
                if let Expression::AssignmentExpression(a) = &s.expression
                    && let oxc_ast::ast::AssignmentTarget::StaticMemberExpression(m) = &a.left
                    && matches!(&m.object, Expression::Identifier(id) if id.name.as_str() == "module")
                    && m.property.name.as_str() == "exports"
                {
                    return Some(&a.right);
                }
            }
            _ => {}
        }
    }
    None
}

struct Evaluator<'c, 'p, 'a> {
    consts: &'c HashMap<String, &'p Expression<'a>>,
    bindings: &'c ModuleBindings,
    /// File name of the config file, which `__filename` and `import.meta.url` name.
    file: &'c str,
}

impl Evaluator<'_, '_, '_> {
    fn expr(&self, expr: &Expression<'_>, depth: u32) -> ConfigValue {
        if depth > MAX_DEPTH {
            return ConfigValue::Unknown;
        }
        let depth = depth + 1;
        let inner = expr.get_inner_expression();
        match inner {
            Expression::NullLiteral(_) => ConfigValue::Null,
            Expression::BooleanLiteral(b) => ConfigValue::Bool(b.value),
            Expression::NumericLiteral(n) => ConfigValue::Number(n.value),
            Expression::StringLiteral(_) => static_string(inner).map_or(ConfigValue::Unknown, ConfigValue::String),
            Expression::TemplateLiteral(template) => self.template(template, depth),
            Expression::BinaryExpression(b) if b.operator == BinaryOperator::Addition => {
                match (self.expr(&b.left, depth), self.expr(&b.right, depth)) {
                    (ConfigValue::String(left), ConfigValue::String(right)) => ConfigValue::String(left + &right),
                    _ => ConfigValue::Unknown,
                }
            }
            Expression::StaticMemberExpression(member) => self.member(member, depth),
            Expression::NewExpression(new) => self.url(new, depth),
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => {
                match self.expr(&u.argument, depth) {
                    ConfigValue::Number(n) => ConfigValue::Number(-n),
                    _ => ConfigValue::Unknown,
                }
            }
            Expression::ArrayExpression(a) => self.array(&a.elements, depth),
            Expression::ObjectExpression(o) => self.object(&o.properties, depth),
            Expression::Identifier(id) => self
                .consts
                .get(id.name.as_str())
                .map_or_else(|| self.global(id.name.as_str()), |init| self.expr(init, depth)),
            Expression::CallExpression(c) => self.call(c, depth),
            Expression::ArrowFunctionExpression(f) => match f.get_expression() {
                Some(body) => self.expr(body, depth),
                None => f
                    .get_function_body()
                    .map_or(ConfigValue::Unknown, |b| self.returned(&b.statements, depth)),
            },
            Expression::FunctionExpression(f) => f
                .body
                .as_ref()
                .map_or(ConfigValue::Unknown, |b| self.returned(&b.statements, depth)),
            _ => ConfigValue::Unknown,
        }
    }

    /// The path of the config file's directory (`__dirname`) or of the file itself (`__filename`).
    fn global(&self, name: &str) -> ConfigValue {
        match name {
            "__dirname" => ConfigValue::String(".".to_owned()),
            "__filename" => ConfigValue::String(format!("./{}", self.file)),
            _ => ConfigValue::Unknown,
        }
    }

    fn template(&self, template: &TemplateLiteral<'_>, depth: u32) -> ConfigValue {
        let mut out = String::new();
        for (index, quasi) in template.quasis.iter().enumerate() {
            out.push_str(
                &quasi
                    .value
                    .cooked
                    .map_or_else(|| quasi.value.raw.as_str().to_owned(), crate::js_string),
            );
            if let Some(expression) = template.expressions.get(index) {
                let ConfigValue::String(text) = self.expr(expression, depth) else {
                    return ConfigValue::Unknown;
                };
                out.push_str(&text);
            }
        }
        ConfigValue::String(out)
    }

    /// `import.meta.dirname`, `import.meta.filename`, `import.meta.url` and the `pathname` of a `new URL(…)`.
    fn member(&self, member: &StaticMemberExpression<'_>, depth: u32) -> ConfigValue {
        let property = member.property.name.as_str();
        match (member.object.get_inner_expression(), property) {
            (Expression::ImportMeta(_), "dirname") => ConfigValue::String(".".to_owned()),
            (Expression::ImportMeta(_), "filename" | "url") => ConfigValue::String(format!("./{}", self.file)),
            (object @ Expression::NewExpression(_), "pathname" | "href") => self.expr(object, depth),
            _ => ConfigValue::Unknown,
        }
    }

    /// `new URL('./x', import.meta.url)` as the path it names.
    fn url(&self, new: &NewExpression<'_>, depth: u32) -> ConfigValue {
        let is_url =
            matches!(new.callee.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == "URL");
        let mut args = new.arguments.iter().map(|arg| {
            arg.as_expression()
                .map_or(ConfigValue::Unknown, |e| self.expr(e, depth))
        });
        match (is_url, args.next(), args.next(), args.next()) {
            (true, Some(ConfigValue::String(relative)), Some(ConfigValue::String(base)), None)
                if !relative.starts_with('/') =>
            {
                ConfigValue::String(normalized(&[base.as_str(), "..", relative.as_str()]))
            }
            _ => ConfigValue::Unknown,
        }
    }

    /// A call of a `path` or `url` function, or of `process.cwd()`; `None` for any other call.
    fn path_call(&self, call: &oxc_ast::ast::CallExpression<'_>, depth: u32) -> Option<ConfigValue> {
        let function = match call.callee.get_inner_expression() {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if self.bindings.file_url_to_path.contains(name) {
                    "fileURLToPath"
                } else {
                    self.bindings.path_functions.get(name)?.as_str()
                }
            }
            Expression::StaticMemberExpression(member) => {
                let Expression::Identifier(object) = member.object.get_inner_expression() else {
                    return None;
                };
                let (object, property) = (object.name.as_str(), member.property.name.as_str());
                let path_function = self.bindings.path_modules.contains(object) && PATH_FUNCTIONS.contains(&property);
                let url_function = self.bindings.url_modules.contains(object) && property == "fileURLToPath";
                if path_function || url_function {
                    property
                } else if object == "process" && property == "cwd" {
                    return Some(ConfigValue::String(".".to_owned()));
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        let args: Vec<ConfigValue> = call
            .arguments
            .iter()
            .map(|arg| {
                arg.as_expression()
                    .map_or(ConfigValue::Unknown, |e| self.expr(e, depth))
            })
            .collect();
        Some(apply_path_function(function, &args))
    }

    fn call(&self, call: &oxc_ast::ast::CallExpression<'_>, depth: u32) -> ConfigValue {
        if let Some(value) = self.path_call(call, depth) {
            return value;
        }
        let callee = match call.callee.get_inner_expression() {
            Expression::Identifier(id) => Some(id.name.as_str()),
            Expression::StaticMemberExpression(m) => Some(m.property.name.as_str()),
            _ => None,
        };
        let is_define = callee.is_some_and(|n| n.starts_with("define") && n.len() > "define".len());
        match call.arguments.first().and_then(Argument::as_expression) {
            Some(first) if is_define => self.expr(first, depth),
            _ => ConfigValue::Unknown,
        }
    }

    /// Value returned by a function body that is a single expression or ends in `return <expr>`.
    fn returned(&self, statements: &[Statement<'_>], depth: u32) -> ConfigValue {
        match statements {
            [Statement::ExpressionStatement(s)] => self.expr(&s.expression, depth),
            [.., Statement::ReturnStatement(r)] => r
                .argument
                .as_ref()
                .map_or(ConfigValue::Unknown, |e| self.expr(e, depth)),
            _ => ConfigValue::Unknown,
        }
    }

    fn array(&self, elements: &[ArrayExpressionElement<'_>], depth: u32) -> ConfigValue {
        let mut out = Vec::new();
        for element in elements {
            match element {
                ArrayExpressionElement::SpreadElement(s) => match self.expr(&s.argument, depth) {
                    ConfigValue::Array(items) => out.extend(items),
                    _ => out.push(ConfigValue::Unknown),
                },
                other => out.push(
                    other
                        .as_expression()
                        .map_or(ConfigValue::Unknown, |e| self.expr(e, depth)),
                ),
            }
        }
        ConfigValue::Array(out)
    }

    fn object(&self, properties: &[ObjectPropertyKind<'_>], depth: u32) -> ConfigValue {
        let mut out: Vec<(String, ConfigValue)> = Vec::new();
        for property in properties {
            match property {
                ObjectPropertyKind::SpreadProperty(s) => match self.expr(&s.argument, depth) {
                    ConfigValue::Object(entries) => out.extend(entries),
                    _ => out.push(("...".to_string(), ConfigValue::Unknown)),
                },
                ObjectPropertyKind::ObjectProperty(p) => {
                    let Some(key) = p.key.static_name() else {
                        out.push(("...".to_string(), ConfigValue::Unknown));
                        continue;
                    };
                    let value = if p.method || p.kind != oxc_ast::ast::PropertyKind::Init {
                        ConfigValue::Unknown
                    } else {
                        self.expr(&p.value, depth)
                    };
                    out.push((crate::js_string(key.as_js_str()), value));
                }
            }
        }
        ConfigValue::Object(out)
    }
}
