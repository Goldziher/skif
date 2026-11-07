//! Static evaluation of the value a tool configuration file exports. Nothing is executed.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionExpression, AssignmentOperator, AssignmentTarget, BinaryOperator,
    BindingPattern, CallExpression, Declaration, ExportDefaultDeclarationKind, Expression, Function,
    ImportDeclarationSpecifier, NewExpression, ObjectProperty, ObjectPropertyKind, Program, Statement,
    StaticMemberExpression, TaggedTemplateExpression, TemplateLiteral, UnaryOperator, VariableDeclaration,
    VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::SourceType;
use serde::{Deserialize, Serialize};

use crate::finder::static_string;

const MAX_DEPTH: u32 = 32;

/// How many constants may be evaluated inside one another; a longer chain is unknown.
const MAX_CONST_CHAIN: usize = 64;

/// The largest value a constant may evaluate to. Constants that reference each other twice at every level
/// (`const b = [a, a]; const c = [b, b]; …`) double in size per level; a value past this is unknown.
const MAX_CONST_NODES: usize = 10_000;

/// How many function calls one evaluation may make; a function that calls itself twice at every level is unknown
/// past this.
const MAX_CALLS: usize = 256;

/// Packages whose default export or named exports list files by glob.
const GLOB_PACKAGES: &[&str] = &["glob", "fast-glob", "globby", "tinyglobby"];

/// Packages whose `merge` and `mergeWith` merge configuration objects. Any other module's `merge` (an `RxJS` stream
/// combinator) is not a config combinator.
const MERGE_PACKAGES: &[&str] = &[
    "lodash",
    "lodash-es",
    "lodash/merge",
    "lodash/mergeWith",
    "lodash.merge",
    "lodash.mergewith",
    "deepmerge",
    "deepmerge-ts",
    "ts-deepmerge",
    "merge",
    "merge-deep",
    "webpack-merge",
    "defu",
];

/// Names a combinator call may use that other libraries use for unrelated functions.
const AMBIGUOUS_MERGES: &[&str] = &["merge", "mergeWith"];

/// Members of a glob function that list files too (`fg.sync`, `glob.globSync`).
const GLOB_MEMBERS: &[&str] = &["sync", "async", "glob", "globSync", "globbySync", "stream"];

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
    /// A regular expression literal: its pattern, without the slashes and flags.
    Regex(String),
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

    /// The pattern when this is a regular expression literal.
    #[must_use]
    pub fn as_regex(&self) -> Option<&str> {
        match self {
            Self::Regex(pattern) => Some(pattern),
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
/// the config file's directory (`"."`, `"./src"`, `"../shared"`).
///
/// Functions are followed: a returned config function may declare local `const`/`let` bindings and local functions,
/// assign members of a local object, and call top-level or local functions with known arguments; `await x` and
/// `Promise.resolve(x)` evaluate to `x`; `require.resolve('pkg')` evaluates to `'pkg'`, and the storybook idiom
/// `dirname(require.resolve(join('pkg', 'package.json')))` to `'pkg'`. A call of a glob function (`glob`,
/// `fast-glob`, `globby`, `tinyglobby`, however imported) evaluates to the array of its literal patterns, so a
/// dynamic `entry` becomes entry globs. `process.cwd()` and a first `path.resolve` argument that is not absolute are
/// taken to be the config file's directory, which is right when the tool runs from the package that holds the
/// config and a heuristic otherwise. The directory itself is `"."`, so `dirname(__dirname)` is `".."`. A URL with a
/// scheme (`new URL(x, 'https://…')`, `fileURLToPath('file:///…')`) and `.href` are [`ConfigValue::Unknown`], as is
/// everything else. Returns `None` when the file does not parse or exports nothing recognisable.
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
    let top = top_level(program);
    let bindings = module_bindings(program);
    let file = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let eval = Evaluator {
        consts: &top.consts,
        functions: &top.functions,
        bindings: &bindings,
        file,
        evaluated: RefCell::new(HashMap::new()),
        chain: RefCell::new(Vec::new()),
        frames: RefCell::new(Vec::new()),
        calls: Cell::new(0),
        config_setter: RefCell::new(None),
        captured: RefCell::new(None),
    };
    let exported = exported(program)?;
    let callable = match exported {
        Exported::Expression(expr) => Callable::of_expression(expr),
        Exported::Function(function) => Some(Callable::Function(function)),
    };
    *eval.config_setter.borrow_mut() = callable.and_then(first_parameter);
    let value = match exported {
        Exported::Expression(expr) => eval.expr(expr, 0),
        Exported::Function(function) => eval.invoke(Callable::Function(function), &[], false, 0),
    };
    let captured = eval.captured.borrow_mut().take();
    Some(captured.unwrap_or(value))
}

/// Name of the first parameter of `callable`, when it is a plain identifier.
fn first_parameter(callable: Callable<'_, '_>) -> Option<String> {
    let params = match callable {
        Callable::Function(f) => &f.params,
        Callable::Arrow(f) => &f.params,
    };
    match &params.items.first()?.pattern {
        BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_owned()),
        _ => None,
    }
}

/// The top-level `const` initialisers and function declarations of a module.
struct TopLevel<'p, 'a> {
    consts: HashMap<String, &'p Expression<'a>>,
    functions: HashMap<String, Callable<'p, 'a>>,
}

/// A function the evaluator can call.
#[derive(Clone, Copy)]
enum Callable<'p, 'a> {
    Function(&'p Function<'a>),
    Arrow(&'p ArrowFunctionExpression<'a>),
}

impl<'p, 'a> Callable<'p, 'a> {
    fn of_expression(expr: &'p Expression<'a>) -> Option<Self> {
        match expr.get_inner_expression() {
            Expression::ArrowFunctionExpression(f) => Some(Self::Arrow(f)),
            Expression::FunctionExpression(f) => Some(Self::Function(f)),
            _ => None,
        }
    }
}

/// What a name is bound to inside a function being evaluated.
#[derive(Clone)]
enum Binding<'p, 'a> {
    Value(ConfigValue),
    Callable(Callable<'p, 'a>),
}

type Frame<'p, 'a> = HashMap<String, Binding<'p, 'a>>;

fn top_level<'p, 'a>(program: &'p Program<'a>) -> TopLevel<'p, 'a> {
    let mut top = TopLevel {
        consts: HashMap::new(),
        functions: HashMap::new(),
    };
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        match decl {
            Some(Declaration::VariableDeclaration(v)) if v.kind == VariableDeclarationKind::Const => {
                for d in &v.declarations {
                    if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init) {
                        top.consts.insert(id.name.as_str().to_string(), init);
                        if let Some(callable) = Callable::of_expression(init) {
                            top.functions.insert(id.name.as_str().to_string(), callable);
                        }
                    }
                }
            }
            Some(Declaration::FunctionDeclaration(f)) => {
                if let Some(id) = &f.id {
                    top.functions
                        .insert(id.name.as_str().to_string(), Callable::Function(f));
                }
            }
            _ => {}
        }
    }
    top
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
    /// Local names of glob functions and glob modules.
    glob_names: HashSet<String>,
    /// Local names bound to a package that merges configuration objects.
    merge_names: HashSet<String>,
    /// Local `merge` and `mergeWith` imported from a package that does something else with them.
    foreign_merges: HashSet<String>,
}

impl ModuleBindings {
    /// Records `local` as the module itself (`member` is `None`) or as its export `member`.
    fn bind(&mut self, module: &str, member: Option<&str>, local: &str) {
        let local = local.to_owned();
        if MERGE_PACKAGES.contains(&module) {
            self.merge_names.insert(local.clone());
        } else if AMBIGUOUS_MERGES.contains(&local.as_str()) {
            self.foreign_merges.insert(local.clone());
        }
        match (module.strip_prefix("node:").unwrap_or(module), member) {
            ("path" | "path/posix", None | Some("default")) => {
                self.path_modules.insert(local);
            }
            ("path" | "path/posix", Some(name)) if PATH_FUNCTIONS.contains(&name) => {
                self.path_functions.insert(local, name.to_owned());
            }
            ("url", None | Some("default")) => {
                self.url_modules.insert(local);
            }
            ("url", Some("fileURLToPath")) => {
                self.file_url_to_path.insert(local);
            }
            (package, _) if GLOB_PACKAGES.contains(&package) => {
                self.glob_names.insert(local);
            }
            ("fs", Some("globSync")) | ("fs/promises", Some("glob")) => {
                self.glob_names.insert(local);
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
        // Only a path derived from `import.meta.url` is known; a URL with a scheme names something else.
        ("fileURLToPath", [file]) if !file.contains(':') => (*file).to_owned(),
        ("dirname", [path]) => normalized(&[*path, ".."]),
        ("normalize", [path]) => normalized(&[*path]),
        ("resolve", parts) if !parts.iter().any(absolute) => normalized(parts),
        ("join", parts) if !parts.first().is_some_and(absolute) => normalized(parts),
        _ => return ConfigValue::Unknown,
    };
    ConfigValue::String(path)
}

/// Statically evaluates the option `key` of every `new constructor({ … })` in `source`, however the constructor was
/// imported (`new NodejsFunction(…)`, `new lambda_nodejs.NodejsFunction(…)`). Path computations are relative to the
/// directory of `path`. Returns nothing when the file does not parse.
#[must_use]
pub fn extract_constructor_options(source: &str, path: &Path, constructor: &str, key: &str) -> Vec<ConfigValue> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let program = &ret.program;
    let top = top_level(program);
    let bindings = module_bindings(program);
    let mut finder = OptionFinder {
        top: &top,
        bindings: &bindings,
        file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        constructor,
        key,
        found: Vec::new(),
    };
    finder.visit_program(program);
    finder.found
}

/// Statically evaluates the value of every property named `key` of every object literal in `source`, however deeply
/// nested and wherever it appears (`handler: "src/api.handler"` inside a function body), in source order. Values are
/// evaluated as in [`extract_config_object`], with paths relative to the directory of `path`. Methods are left out.
/// Returns nothing when the file does not parse.
#[must_use]
pub fn extract_property_values(source: &str, path: &Path, key: &str) -> Vec<ConfigValue> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let program = &ret.program;
    let top = top_level(program);
    let bindings = module_bindings(program);
    let mut finder = PropertyFinder {
        top: &top,
        bindings: &bindings,
        file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        key,
        found: Vec::new(),
    };
    finder.visit_program(program);
    finder.found
}

/// A call, or a tagged template, of a function a module exports; see [`extract_imported_calls`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedCall {
    /// The module the function comes from.
    pub module: String,
    /// The export called: `"default"` for the default export or the module object itself (`pino(…)`), the export's
    /// name for a named import (`execa(…)`), or the member called on the module object (`pino.transport(…)`).
    pub export: String,
    /// The statically evaluated arguments, in order; empty for a tagged template.
    pub arguments: Vec<ConfigValue>,
    /// For a tagged template (`` $`cmd ${x}` ``), its raw text with every substitution replaced by
    /// [`TEMPLATE_SUBSTITUTION`]; `None` for a call.
    pub template: Option<String>,
}

/// What a substitution of a tagged template reads as in [`ImportedCall::template`].
pub const TEMPLATE_SUBSTITUTION: &str = "$__skif__";

/// Every call and tagged template in `source` whose function is imported from one of `modules` (`import`, `import * as`,
/// or a top-level `require`, plain or destructured), with its arguments statically evaluated as in
/// [`extract_config_object`]. `globals` names functions available without an import, as `(name, module)` pairs (the
/// `$` of a zx script). Shadowed names are not tracked. Returns nothing when the file does not parse.
#[must_use]
pub fn extract_imported_calls(
    source: &str,
    path: &Path,
    modules: &[&str],
    globals: &[(&str, &str)],
) -> Vec<ImportedCall> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let program = &ret.program;
    let imported = imported_functions(program, modules, globals);
    if imported.is_empty() {
        return Vec::new();
    }
    let top = top_level(program);
    let bindings = module_bindings(program);
    let mut finder = CallFinder {
        top: &top,
        bindings: &bindings,
        file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        imported: &imported,
        found: Vec::new(),
    };
    finder.visit_program(program);
    finder.found
}

/// Local name to module and named export (`None` for the default export or the module object) of everything a
/// program imports from `modules`, plus `globals`.
type Imported = HashMap<String, (String, Option<String>)>;

fn imported_functions(program: &Program<'_>, modules: &[&str], globals: &[(&str, &str)]) -> Imported {
    let mut imported: Imported = globals
        .iter()
        .map(|(name, module)| ((*name).to_owned(), ((*module).to_owned(), Some((*name).to_owned()))))
        .collect();
    let named = |export: String| Some(export).filter(|e| e != "default");
    for stmt in &program.body {
        if let Statement::ImportDeclaration(d) = stmt {
            let module = d.source.value.as_str().unwrap_or_default();
            if !modules.contains(&module) {
                continue;
            }
            for specifier in d.specifiers.iter().flatten() {
                let (local, export) = match specifier {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => (
                        s.local.name.as_str(),
                        named(crate::collect::module_export_name(&s.imported)),
                    ),
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => (s.local.name.as_str(), None),
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => (s.local.name.as_str(), None),
                };
                imported.insert(local.to_owned(), (module.to_owned(), export));
            }
        } else if let Some(Declaration::VariableDeclaration(v)) = stmt.as_declaration() {
            for declarator in &v.declarations {
                if let Some(module) = declarator.init.as_ref().and_then(required_module)
                    && modules.contains(&module.as_str())
                {
                    for (local, export) in required_names(&declarator.id) {
                        imported.insert(local, (module.clone(), export.and_then(named)));
                    }
                }
            }
        }
    }
    imported
}

/// The local names a `require` result is bound to, with the export each destructured one takes.
fn required_names(id: &BindingPattern<'_>) -> Vec<(String, Option<String>)> {
    match id {
        BindingPattern::BindingIdentifier(local) => vec![(local.name.as_str().to_owned(), None)],
        BindingPattern::ObjectPattern(pattern) => pattern
            .properties
            .iter()
            .filter_map(|property| match (property.key.static_name(), &property.value) {
                (Some(key), BindingPattern::BindingIdentifier(local)) => {
                    Some((local.name.as_str().to_owned(), Some(crate::js_string(key.as_js_str()))))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

struct CallFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    /// Local name to module and named export (`None` for the default export or the module object).
    imported: &'t Imported,
    found: Vec<ImportedCall>,
}

impl<'a> CallFinder<'_, '_, 'a> {
    /// The module and export `callee` calls, when it is an imported function or a member of an imported module.
    fn target(&self, callee: &Expression<'a>) -> Option<(String, String)> {
        match callee.get_inner_expression() {
            Expression::Identifier(id) => self
                .imported
                .get(id.name.as_str())
                .map(|(module, export)| (module.clone(), export.clone().unwrap_or_else(|| "default".to_owned()))),
            Expression::StaticMemberExpression(member) => match member.object.get_inner_expression() {
                Expression::Identifier(id) => self
                    .imported
                    .get(id.name.as_str())
                    .filter(|(_, export)| export.is_none())
                    .map(|(module, _)| (module.clone(), member.property.name.as_str().to_owned())),
                _ => None,
            },
            _ => None,
        }
    }
}

impl<'a> Visit<'a> for CallFinder<'_, '_, 'a> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some((module, export)) = self.target(&it.callee) {
            let eval = Evaluator {
                consts: &self.top.consts,
                functions: &self.top.functions,
                bindings: self.bindings,
                file: self.file,
                evaluated: RefCell::new(HashMap::new()),
                chain: RefCell::new(Vec::new()),
                frames: RefCell::new(Vec::new()),
                calls: Cell::new(0),
                config_setter: RefCell::new(None),
                captured: RefCell::new(None),
            };
            let arguments = it
                .arguments
                .iter()
                .map(|argument| {
                    argument
                        .as_expression()
                        .map_or(ConfigValue::Unknown, |e| eval.expr(e, 0))
                })
                .collect();
            self.found.push(ImportedCall {
                module,
                export,
                arguments,
                template: None,
            });
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        let tag = match it.tag.get_inner_expression() {
            Expression::CallExpression(call) => &call.callee,
            other => other,
        };
        if let Some((module, export)) = self.target(tag) {
            let quasis: Vec<&str> = it.quasi.quasis.iter().map(|q| q.value.raw.as_str()).collect();
            self.found.push(ImportedCall {
                module,
                export,
                arguments: Vec::new(),
                template: Some(quasis.join(TEMPLATE_SUBSTITUTION)),
            });
        }
        walk::walk_tagged_template_expression(self, it);
    }
}

struct OptionFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    constructor: &'t str,
    key: &'t str,
    found: Vec<ConfigValue>,
}

impl<'a> Visit<'a> for OptionFinder<'_, '_, 'a> {
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        let name = match it.callee.get_inner_expression() {
            Expression::Identifier(id) => Some(id.name.as_str()),
            Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
            _ => None,
        };
        if name == Some(self.constructor) {
            let eval = Evaluator {
                consts: &self.top.consts,
                functions: &self.top.functions,
                bindings: self.bindings,
                file: self.file,
                evaluated: RefCell::new(HashMap::new()),
                chain: RefCell::new(Vec::new()),
                frames: RefCell::new(Vec::new()),
                calls: Cell::new(0),
                config_setter: RefCell::new(None),
                captured: RefCell::new(None),
            };
            for argument in &it.arguments {
                let Some(Expression::ObjectExpression(options)) =
                    argument.as_expression().map(Expression::get_inner_expression)
                else {
                    continue;
                };
                for property in &options.properties {
                    if let ObjectPropertyKind::ObjectProperty(p) = property
                        && !p.method
                        && p.key
                            .static_name()
                            .is_some_and(|name| crate::js_string(name.as_js_str()) == self.key)
                    {
                        self.found.push(eval.expr(&p.value, 0));
                    }
                }
            }
        }
        walk::walk_new_expression(self, it);
    }
}

struct PropertyFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    key: &'t str,
    found: Vec<ConfigValue>,
}

impl<'a> Visit<'a> for PropertyFinder<'_, '_, 'a> {
    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        if !it.method
            && it
                .key
                .static_name()
                .is_some_and(|name| crate::js_string(name.as_js_str()) == self.key)
        {
            let eval = Evaluator {
                consts: &self.top.consts,
                functions: &self.top.functions,
                bindings: self.bindings,
                file: self.file,
                evaluated: RefCell::new(HashMap::new()),
                chain: RefCell::new(Vec::new()),
                frames: RefCell::new(Vec::new()),
                calls: Cell::new(0),
                config_setter: RefCell::new(None),
                captured: RefCell::new(None),
            };
            self.found.push(eval.expr(&it.value, 0));
        }
        walk::walk_object_property(self, it);
    }
}

/// What a module exports.
#[derive(Clone, Copy)]
enum Exported<'p, 'a> {
    Expression(&'p Expression<'a>),
    Function(&'p Function<'a>),
}

fn exported<'p, 'a>(program: &'p Program<'a>) -> Option<Exported<'p, 'a>> {
    for stmt in &program.body {
        match stmt {
            Statement::ExportDefaultDeclaration(e) => {
                return match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => Some(Exported::Function(f)),
                    ExportDefaultDeclarationKind::ClassDeclaration(_)
                    | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => None,
                    other => other.as_expression().map(Exported::Expression),
                };
            }
            Statement::TSExportAssignment(e) => return Some(Exported::Expression(&e.expression)),
            Statement::ExpressionStatement(s) => {
                if let Expression::AssignmentExpression(a) = &s.expression
                    && let AssignmentTarget::StaticMemberExpression(m) = &a.left
                    && matches!(&m.object, Expression::Identifier(id) if id.name.as_str() == "module")
                    && m.property.name.as_str() == "exports"
                {
                    return Some(Exported::Expression(&a.right));
                }
            }
            _ => {}
        }
    }
    None
}

/// The number of values `value` consists of, itself included.
fn node_count(value: &ConfigValue) -> usize {
    1 + match value {
        ConfigValue::Array(items) => items.iter().map(node_count).sum(),
        ConfigValue::Object(entries) => entries.iter().map(|(_, item)| node_count(item)).sum(),
        _ => 0,
    }
}

struct Evaluator<'c, 'p, 'a> {
    consts: &'c HashMap<String, &'p Expression<'a>>,
    functions: &'c HashMap<String, Callable<'p, 'a>>,
    bindings: &'c ModuleBindings,
    /// File name of the config file, which `__filename` and `import.meta.url` name.
    file: &'c str,
    /// The value of every top-level constant evaluated so far.
    evaluated: RefCell<HashMap<String, ConfigValue>>,
    /// The constants being evaluated, outermost first; a constant that refers back to one of them is unknown.
    chain: RefCell<Vec<String>>,
    /// The bindings of the functions being evaluated, outermost first.
    frames: RefCell<Vec<Frame<'p, 'a>>>,
    /// How many functions have been called so far.
    calls: Cell<usize>,
    /// The first parameter of an exported config function (`config` in `module.exports = function (config) { … }`):
    /// the object the function hands the real config to with `config.set({ … })`.
    config_setter: RefCell<Option<String>>,
    /// The argument of the last `<config_setter>.set(…)` call evaluated.
    captured: RefCell<Option<ConfigValue>>,
}

impl<'p, 'a> Evaluator<'_, 'p, 'a> {
    fn expr(&self, expr: &'p Expression<'a>, depth: u32) -> ConfigValue {
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
            Expression::RegExpLiteral(r) => ConfigValue::Regex(r.regex.pattern.text.as_str().to_owned()),
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
            Expression::AwaitExpression(a) => self.expr(&a.argument, depth),
            Expression::ArrayExpression(a) => self.array(&a.elements, depth),
            Expression::ObjectExpression(o) => self.object(&o.properties, depth),
            Expression::Identifier(id) => self.identifier(id.name.as_str(), depth),
            Expression::CallExpression(c) => self.call(c, depth),
            Expression::ArrowFunctionExpression(f) => self.invoke(Callable::Arrow(f), &[], false, depth),
            Expression::FunctionExpression(f) => self.invoke(Callable::Function(f), &[], false, depth),
            _ => ConfigValue::Unknown,
        }
    }

    /// The value of a name: a binding of a function being evaluated, a top-level constant (evaluated once however
    /// often it is referenced), a top-level function called without arguments, or a path global.
    fn identifier(&self, name: &str, depth: u32) -> ConfigValue {
        if let Some(binding) = self.lookup(name) {
            return match binding {
                Binding::Value(value) => value,
                Binding::Callable(_) => ConfigValue::Unknown,
            };
        }
        let Some(&init) = self.consts.get(name) else {
            return match self.functions.get(name) {
                Some(callable) => self.invoke(*callable, &[], true, depth),
                None => self.global(name),
            };
        };
        if let Some(known) = self.evaluated.borrow().get(name) {
            return known.clone();
        }
        {
            let chain = self.chain.borrow();
            if chain.len() >= MAX_CONST_CHAIN || chain.iter().any(|active| active == name) {
                return ConfigValue::Unknown;
            }
        }
        self.chain.borrow_mut().push(name.to_owned());
        // The value does not depend on where it is referenced from: evaluate it outside any function's bindings and
        // from a fresh depth budget.
        let saved = std::mem::take(&mut *self.frames.borrow_mut());
        let value = self.expr(init, 0);
        *self.frames.borrow_mut() = saved;
        self.chain.borrow_mut().pop();
        let value = if node_count(&value) > MAX_CONST_NODES {
            ConfigValue::Unknown
        } else {
            value
        };
        self.evaluated.borrow_mut().insert(name.to_owned(), value.clone());
        value
    }

    /// The innermost binding of `name` in the functions being evaluated.
    fn lookup(&self, name: &str) -> Option<Binding<'p, 'a>> {
        self.frames
            .borrow()
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).cloned())
    }

    /// Binds `name` in the innermost frame, or replaces the binding where it is already made.
    fn assign(&self, name: &str, binding: Binding<'p, 'a>) {
        let mut frames = self.frames.borrow_mut();
        if let Some(frame) = frames.iter_mut().rev().find(|frame| frame.contains_key(name)) {
            frame.insert(name.to_owned(), binding);
        } else if let Some(frame) = frames.last_mut() {
            frame.insert(name.to_owned(), binding);
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

    fn template(&self, template: &'p TemplateLiteral<'a>, depth: u32) -> ConfigValue {
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
    fn member(&self, member: &'p StaticMemberExpression<'a>, depth: u32) -> ConfigValue {
        let property = member.property.name.as_str();
        match (member.object.get_inner_expression(), property) {
            (Expression::ImportMeta(_), "dirname") => ConfigValue::String(".".to_owned()),
            (Expression::ImportMeta(_), "filename" | "url") => ConfigValue::String(format!("./{}", self.file)),
            (object @ Expression::NewExpression(_), "pathname") => self.expr(object, depth),
            _ => ConfigValue::Unknown,
        }
    }

    /// `new URL('./x', import.meta.url)` as the path it names.
    fn url(&self, new: &'p NewExpression<'a>, depth: u32) -> ConfigValue {
        let is_url =
            matches!(new.callee.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == "URL");
        let mut args = new.arguments.iter().map(|arg| {
            arg.as_expression()
                .map_or(ConfigValue::Unknown, |e| self.expr(e, depth))
        });
        match (is_url, args.next(), args.next(), args.next()) {
            (true, Some(ConfigValue::String(relative)), Some(ConfigValue::String(base)), None)
                if !relative.starts_with('/') && !relative.contains(':') && !base.contains(':') =>
            {
                ConfigValue::String(normalized(&[base.as_str(), "..", relative.as_str()]))
            }
            _ => ConfigValue::Unknown,
        }
    }

    /// Whether `callee` is the function `wanted` of the `path` module, however it was imported.
    fn is_path_function(&self, callee: &Expression<'_>, wanted: &str) -> bool {
        match callee.get_inner_expression() {
            Expression::Identifier(id) => self
                .bindings
                .path_functions
                .get(id.name.as_str())
                .is_some_and(|function| function == wanted),
            Expression::StaticMemberExpression(member) => {
                member.property.name.as_str() == wanted
                    && matches!(
                        member.object.get_inner_expression(),
                        Expression::Identifier(object) if self.bindings.path_modules.contains(object.name.as_str())
                    )
            }
            _ => false,
        }
    }

    /// A call of a `path` or `url` function, or of `process.cwd()`; `None` for any other call.
    fn path_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
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
                // `path.posix.join` computes like `path.join` for the relative paths a config holds.
                let object = match member.object.get_inner_expression() {
                    Expression::StaticMemberExpression(inner) if inner.property.name.as_str() == "posix" => {
                        inner.object.get_inner_expression()
                    }
                    other => other,
                };
                let Expression::Identifier(object) = object else {
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
        Some(apply_path_function(function, &self.arguments(call, depth)))
    }

    /// The values of a call's arguments; a spread argument is unknown.
    fn arguments(&self, call: &'p CallExpression<'a>, depth: u32) -> Vec<ConfigValue> {
        call.arguments
            .iter()
            .map(|arg| {
                arg.as_expression()
                    .map_or(ConfigValue::Unknown, |e| self.expr(e, depth))
            })
            .collect()
    }

    /// `dirname(require.resolve(join(pkg, 'package.json')))`, the directory of a package, which is `pkg` itself.
    fn package_directory(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        if !self.is_path_function(&call.callee, "dirname") {
            return None;
        }
        let Some(Expression::CallExpression(resolve)) = single_argument(call).map(Expression::get_inner_expression)
        else {
            return None;
        };
        if !is_require_resolve(resolve) {
            return None;
        }
        let Some(Expression::CallExpression(join)) = single_argument(resolve).map(Expression::get_inner_expression)
        else {
            return None;
        };
        if !self.is_path_function(&join.callee, "join") {
            return None;
        }
        let mut arguments = join.arguments.iter().filter_map(Argument::as_expression);
        let (package, file) = (arguments.next()?, arguments.next()?);
        if arguments.next().is_some() || static_string(file).as_deref() != Some("package.json") {
            return None;
        }
        Some(self.expr(package, depth))
    }

    /// `require.resolve(x)` and `Promise.resolve(x)`, which evaluate to `x`.
    fn resolve_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        let Expression::Identifier(object) = member.object.get_inner_expression() else {
            return None;
        };
        let known = matches!(
            (object.name.as_str(), member.property.name.as_str()),
            ("require" | "Promise", "resolve")
        );
        known.then(|| single_argument(call).map_or(ConfigValue::Unknown, |arg| self.expr(arg, depth)))
    }

    /// A call of a glob function as the array of its literal patterns.
    fn glob_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let is_glob = match call.callee.get_inner_expression() {
            Expression::Identifier(id) => self.bindings.glob_names.contains(id.name.as_str()),
            Expression::StaticMemberExpression(member) => {
                GLOB_MEMBERS.contains(&member.property.name.as_str())
                    && matches!(
                        member.object.get_inner_expression(),
                        Expression::Identifier(object) if self.bindings.glob_names.contains(object.name.as_str())
                    )
            }
            _ => false,
        };
        if !is_glob {
            return None;
        }
        Some(match call.arguments.first().and_then(Argument::as_expression) {
            Some(patterns) => match self.expr(patterns, depth) {
                ConfigValue::String(pattern) => ConfigValue::Array(vec![ConfigValue::String(pattern)]),
                array @ ConfigValue::Array(_) => array,
                _ => ConfigValue::Unknown,
            },
            None => ConfigValue::Unknown,
        })
    }

    /// A call of a top-level function or of a function declared inside the function being evaluated.
    fn local_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let Expression::Identifier(id) = call.callee.get_inner_expression() else {
            return None;
        };
        let name = id.name.as_str();
        let (callable, isolate) = match self.lookup(name) {
            Some(Binding::Callable(callable)) => (callable, false),
            Some(Binding::Value(_)) => return None,
            None => (*self.functions.get(name)?, true),
        };
        Some(self.invoke(callable, &self.arguments(call, depth), isolate, depth))
    }

    /// A call of a config combinator (`mergeConfig`, `defu`, `Object.assign`, `deepmerge`, `merge`): the merge of the
    /// object arguments. An argument that cannot be known contributes a `...` entry, so the known part still applies.
    fn merge_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let style = merge_style(&call.callee, self.bindings)?;
        let mut merged: Option<Vec<(String, ConfigValue)>> = None;
        let mut values = self.arguments(call, depth);
        if style == MergeStyle::FirstWins {
            values.reverse();
        }
        for value in values {
            match value {
                ConfigValue::Null | ConfigValue::Bool(_) => {}
                ConfigValue::Object(entries) => {
                    let base = merged.get_or_insert_with(Vec::new);
                    merge_objects(base, entries, style != MergeStyle::Shallow);
                }
                _ => merge_objects(
                    merged.get_or_insert_with(Vec::new),
                    vec![("...".to_owned(), ConfigValue::Unknown)],
                    false,
                ),
            }
        }
        Some(merged.map_or(ConfigValue::Unknown, ConfigValue::Object))
    }

    /// Records the argument of `<config_setter>.set(…)` as the config; returns whether `call` is such a call.
    fn capture_set(&self, call: &'p CallExpression<'a>, depth: u32) -> bool {
        let Some(setter) = self.config_setter.borrow().clone() else {
            return false;
        };
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return false;
        };
        let on_setter =
            matches!(member.object.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == setter);
        let Some(argument) = single_argument(call).filter(|_| on_setter && member.property.name.as_str() == "set")
        else {
            return false;
        };
        let value = self.expr(argument, depth);
        *self.captured.borrow_mut() = Some(value);
        true
    }

    fn call(&self, call: &'p CallExpression<'a>, depth: u32) -> ConfigValue {
        if self.capture_set(call, depth) {
            return ConfigValue::Unknown;
        }
        let special = required_package(call, depth)
            .or_else(|| self.package_directory(call, depth))
            .or_else(|| self.path_call(call, depth))
            .or_else(|| self.resolve_call(call, depth))
            .or_else(|| self.glob_call(call, depth))
            .or_else(|| self.local_call(call, depth))
            .or_else(|| self.merge_call(call, depth));
        if let Some(value) = special {
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

    /// Calls `callable` with `args`. With `isolate`, the function cannot see the bindings of the function evaluating it.
    fn invoke(&self, callable: Callable<'p, 'a>, args: &[ConfigValue], isolate: bool, depth: u32) -> ConfigValue {
        if self.calls.get() >= MAX_CALLS {
            return ConfigValue::Unknown;
        }
        self.calls.set(self.calls.get() + 1);
        let params = match callable {
            Callable::Function(f) => &f.params,
            Callable::Arrow(f) => &f.params,
        };
        let mut frame = Frame::new();
        for (index, param) in params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                let value = args.get(index).cloned().unwrap_or(ConfigValue::Unknown);
                frame.insert(id.name.as_str().to_owned(), Binding::Value(value));
            }
        }
        let saved = if isolate {
            std::mem::take(&mut *self.frames.borrow_mut())
        } else {
            Vec::new()
        };
        self.frames.borrow_mut().push(frame);
        let value = match callable {
            Callable::Arrow(f) => match f.get_expression() {
                Some(body) => self.expr(body, depth),
                None => f
                    .get_function_body()
                    .map_or(ConfigValue::Unknown, |b| self.returned(&b.statements, depth)),
            },
            Callable::Function(f) => f
                .body
                .as_ref()
                .map_or(ConfigValue::Unknown, |b| self.returned(&b.statements, depth)),
        };
        self.frames.borrow_mut().pop();
        if isolate {
            *self.frames.borrow_mut() = saved;
        }
        value
    }

    /// Value returned by a function body: its single expression or the first `return <expr>`. Declarations before
    /// it are bound in the innermost frame.
    fn returned(&self, statements: &'p [Statement<'a>], depth: u32) -> ConfigValue {
        if let [Statement::ExpressionStatement(s)] = statements {
            return self.expr(&s.expression, depth);
        }
        for statement in statements {
            match statement {
                Statement::ReturnStatement(r) => {
                    return r
                        .argument
                        .as_ref()
                        .map_or(ConfigValue::Unknown, |e| self.expr(e, depth));
                }
                Statement::ExpressionStatement(s) => self.mutation(&s.expression, depth),
                other => match other.as_declaration() {
                    Some(Declaration::VariableDeclaration(v)) => self.declare(v, depth),
                    Some(Declaration::FunctionDeclaration(f)) => {
                        if let Some(id) = &f.id {
                            self.assign(id.name.as_str(), Binding::Callable(Callable::Function(f)));
                        }
                    }
                    _ => {}
                },
            }
        }
        ConfigValue::Unknown
    }

    /// Binds the names a `const`/`let`/`var` declaration declares; destructured names stay unbound.
    fn declare(&self, declaration: &'p VariableDeclaration<'a>, depth: u32) {
        for declarator in &declaration.declarations {
            let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                continue;
            };
            if let Some(callable) = declarator.init.as_ref().and_then(Callable::of_expression) {
                self.assign(id.name.as_str(), Binding::Callable(callable));
                continue;
            }
            let value = declarator
                .init
                .as_ref()
                .map_or(ConfigValue::Unknown, |init| self.expr(init, depth));
            let value = if node_count(&value) > MAX_CONST_NODES {
                ConfigValue::Unknown
            } else {
                value
            };
            self.assign(id.name.as_str(), Binding::Value(value));
        }
    }

    /// Applies what an expression statement does to a local object: `name = x`, `name.key = x`, and anything else
    /// that reaches into the object (`name.list.push(x)`, `name.a.b = x`), which leaves it partly unknown.
    fn mutation(&self, expr: &'p Expression<'a>, depth: u32) {
        match expr.get_inner_expression() {
            Expression::AwaitExpression(a) => self.mutation(&a.argument, depth),
            Expression::AssignmentExpression(a) => {
                let assign = a.operator == AssignmentOperator::Assign;
                match &a.left {
                    AssignmentTarget::AssignmentTargetIdentifier(id) => {
                        let value = if assign {
                            self.expr(&a.right, depth)
                        } else {
                            ConfigValue::Unknown
                        };
                        self.assign(id.name.as_str(), Binding::Value(value));
                    }
                    AssignmentTarget::StaticMemberExpression(m) => {
                        if let Expression::Identifier(object) = m.object.get_inner_expression()
                            && assign
                        {
                            let value = self.expr(&a.right, depth);
                            self.set_key(object.name.as_str(), m.property.name.as_str(), value);
                        } else if let Some(root) = member_root(&m.object) {
                            self.taint(root);
                        }
                    }
                    _ => {}
                }
            }
            Expression::CallExpression(c) => {
                if self.capture_set(c, depth) {
                    return;
                }
                let object_assign = matches!(
                    c.callee.get_inner_expression(),
                    Expression::StaticMemberExpression(m)
                        if m.property.name.as_str() == "assign"
                            && matches!(m.object.get_inner_expression(), Expression::Identifier(o) if o.name.as_str() == "Object")
                );
                let target = if object_assign {
                    c.arguments.first().and_then(Argument::as_expression)
                } else if let Expression::StaticMemberExpression(m) = c.callee.get_inner_expression() {
                    Some(&m.object)
                } else {
                    None
                };
                if let Some(root) = target.and_then(member_root) {
                    self.taint(root);
                }
            }
            _ => {}
        }
    }

    /// Sets `key` of the local object `name`.
    fn set_key(&self, name: &str, key: &str, value: ConfigValue) {
        let mut frames = self.frames.borrow_mut();
        let binding = frames.iter_mut().rev().find_map(|frame| frame.get_mut(name));
        if let Some(Binding::Value(ConfigValue::Object(entries))) = binding {
            match entries.iter_mut().find(|(existing, _)| existing == key) {
                Some((_, slot)) => *slot = value,
                None => entries.push((key.to_owned(), value)),
            }
        }
    }

    /// Records that the local `name` was changed in a way the evaluator does not follow.
    fn taint(&self, name: &str) {
        let mut frames = self.frames.borrow_mut();
        let binding = frames.iter_mut().rev().find_map(|frame| frame.get_mut(name));
        match binding {
            Some(Binding::Value(ConfigValue::Object(entries))) => {
                entries.push(("...".to_owned(), ConfigValue::Unknown));
            }
            Some(Binding::Value(ConfigValue::Array(items))) => items.push(ConfigValue::Unknown),
            _ => {}
        }
    }

    fn array(&self, elements: &'p [ArrayExpressionElement<'a>], depth: u32) -> ConfigValue {
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

    fn object(&self, properties: &'p [ObjectPropertyKind<'a>], depth: u32) -> ConfigValue {
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

/// How a config combinator merges its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeStyle {
    /// Nested objects merge and arrays concatenate; later arguments win (`mergeConfig`, `deepmerge`, `merge`).
    Deep,
    /// Top-level keys of later arguments replace earlier ones (`Object.assign`).
    Shallow,
    /// Like [`MergeStyle::Deep`], but earlier arguments win (`defu`).
    FirstWins,
}

/// The merge style of a call to a well-known config combinator, by callee. `merge` and `mergeWith` count only when
/// the file does not import them from another package, and as members only of `lodash` or a merge package.
fn merge_style(callee: &Expression<'_>, bindings: &ModuleBindings) -> Option<MergeStyle> {
    match callee.get_inner_expression() {
        Expression::Identifier(id) => match id.name.as_str() {
            name if bindings.foreign_merges.contains(name) => None,
            "mergeConfig" | "merge" | "deepmerge" | "deepMerge" | "mergeDeep" | "mergeWith" => Some(MergeStyle::Deep),
            "defu" => Some(MergeStyle::FirstWins),
            _ => None,
        },
        Expression::StaticMemberExpression(member) => {
            let object_is = |name: &str| matches!(member.object.get_inner_expression(), Expression::Identifier(o) if o.name.as_str() == name);
            let merge_object = matches!(
                member.object.get_inner_expression(),
                Expression::Identifier(o)
                    if matches!(o.name.as_str(), "_" | "lodash") || bindings.merge_names.contains(o.name.as_str())
            );
            match member.property.name.as_str() {
                "assign" if object_is("Object") => Some(MergeStyle::Shallow),
                "merge" | "mergeWith" if merge_object => Some(MergeStyle::Deep),
                "mergeConfig" | "deepmerge" | "mergeDeep" => Some(MergeStyle::Deep),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Merges `incoming` into `base`; with `deep`, an object meeting an object merges and an array meeting an array
/// concatenates, otherwise the incoming value replaces the earlier one in place.
fn merge_objects(base: &mut Vec<(String, ConfigValue)>, incoming: Vec<(String, ConfigValue)>, deep: bool) {
    for (key, value) in incoming {
        let existing = if key == "..." {
            base.iter().position(|(k, _)| k == "...")
        } else {
            base.iter().position(|(k, _)| *k == key)
        };
        let Some(index) = existing else {
            base.push((key, value));
            continue;
        };
        if key == "..." {
            continue;
        }
        let slot = &mut base[index].1;
        match (deep, std::mem::replace(slot, ConfigValue::Unknown), value) {
            (true, ConfigValue::Object(mut left), ConfigValue::Object(right)) => {
                merge_objects(&mut left, right, true);
                *slot = ConfigValue::Object(left);
            }
            (true, ConfigValue::Array(mut left), ConfigValue::Array(right)) => {
                left.extend(right);
                *slot = ConfigValue::Array(left);
            }
            (_, _, value) => *slot = value,
        }
    }
}

/// The only argument of a call, when it is a plain expression.
fn single_argument<'p, 'a>(call: &'p CallExpression<'a>) -> Option<&'p Expression<'a>> {
    let mut arguments = call.arguments.iter();
    match (arguments.next(), arguments.next()) {
        (Some(only), None) => only.as_expression(),
        _ => None,
    }
}

/// `require('pkg')` and the factory call `require('pkg')(options)` nested in a config value (a `PostCSS` plugin list),
/// as the package name. A top-level `require` stays unknown: it is a whole config, not a name. Relative paths stay
/// unknown too, as they load a module rather than name a package.
fn required_package(call: &CallExpression<'_>, depth: u32) -> Option<ConfigValue> {
    if depth < 2 {
        return None;
    }
    let call = match call.callee.get_inner_expression() {
        Expression::CallExpression(factory) => &**factory,
        _ => call,
    };
    let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
        return None;
    };
    if callee.name.as_str() != "require" {
        return None;
    }
    let module = single_argument(call).and_then(static_string)?;
    (!module.starts_with(['.', '/'])).then_some(ConfigValue::String(module))
}

fn is_require_resolve(call: &CallExpression<'_>) -> bool {
    matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(m)
            if m.property.name.as_str() == "resolve"
                && matches!(m.object.get_inner_expression(), Expression::Identifier(o) if o.name.as_str() == "require")
    )
}

/// The identifier at the start of a member chain (`config` in `config.a.b`).
fn member_root<'a>(expr: &'a Expression<'_>) -> Option<&'a str> {
    match expr.get_inner_expression() {
        Expression::Identifier(id) => Some(id.name.as_str()),
        Expression::StaticMemberExpression(m) => member_root(&m.object),
        _ => None,
    }
}
