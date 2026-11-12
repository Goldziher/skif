//! React Router: the app directory holds the root, entries and the route modules `routes.ts` names.
//!
//! knip runs `routes.ts`; skif reads it as text. The string literals that name a source file are route modules, and a
//! `flatRoutes()` call (`@react-router/fs-routes`) makes the files of the routes directory (`rootDirectory`, `routes`
//! by default) route modules: the route files directly in it and the `route` or `index` file of each folder in it.
//! `ignoredRouteFiles` is not applied, and routes a program computes (`redirects.map(…)`) are not known.

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, looks_like_file, production_entries};

/// The `react-router` plugin.
pub struct ReactRouter;

static SPEC: PluginSpec = PluginSpec {
    id: "react-router",
    enablers: &[Enabler::Dependency("@react-router/dev")],
    config: &["react-router.config.{js,ts}"],
    binaries: &["react-router"],
    ..PluginSpec::DEFAULT
};

const DEFAULT_APP_DIRECTORY: &str = "app";

const ROUTE_EXTENSIONS: &str = "{js,jsx,ts,tsx,md,mdx}";

impl Plugin for ReactRouter {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let directory = config
            .value
            .get("appDirectory")
            .and_then(skif_js_syntax::ConfigValue::as_str)
            .unwrap_or(DEFAULT_APP_DIRECTORY);
        app_inputs(cx, &config.join(directory))
    }

    fn resolve_without_config(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        app_inputs(cx, DEFAULT_APP_DIRECTORY)
    }
}

/// The inputs of an app directory. The route modules are the files `routes.{ts,js}` names with string literals; the
/// default server entry needs `@react-router/node` and `isbot` when the app has none of its own.
fn app_inputs(cx: &PluginContext<'_>, app: &str) -> Vec<PluginInput> {
    let mut out = vec![PluginInput::Entry(join_rel(app, "routes.{js,ts}"))];
    out.extend(production_entries(
        "",
        [
            join_rel(app, "root.{jsx,tsx}"),
            join_rel(app, "entry.{client,server}.{js,jsx,ts,tsx}"),
        ]
        .iter()
        .map(String::as_str),
    ));
    for routes_file in ["routes.ts", "routes.js"] {
        if let Ok(source) = cx.fs.read_to_string(&cx.dir.join(join_rel(app, routes_file))) {
            let files: Vec<String> = quoted_files(&source).iter().map(|file| escape_glob(file)).collect();
            out.extend(production_entries(app, files.iter().map(String::as_str)));
            out.extend(file_system_routes(app, &source));
            break;
        }
    }
    if find_files(
        cx.fs,
        cx.globs,
        cx.dir,
        &[join_rel(app, "entry.server.{js,ts,jsx,tsx}")],
    )
    .is_empty()
    {
        out.push(PluginInput::Dependency("@react-router/node".to_owned()));
        out.push(PluginInput::Dependency("isbot".to_owned()));
    }
    out
}

/// The route modules of a `flatRoutes()` call in `routes.ts`: none without one.
fn file_system_routes(app: &str, source: &str) -> Vec<PluginInput> {
    let Some(call) = source.find("flatRoutes(") else {
        return Vec::new();
    };
    let arguments = &source[call..];
    let directory = arguments
        .find("rootDirectory")
        .and_then(|at| arguments[at..].split_once(':'))
        .and_then(|(_, rest)| {
            let rest = rest.trim_start();
            let quote = rest.chars().next().filter(|c| matches!(*c, '"' | '\'' | '`'))?;
            rest[1..]
                .split_once(quote)
                .map(|(value, _)| value.trim_matches('/').to_owned())
        })
        .unwrap_or_else(|| "routes".to_owned());
    let routes = join_rel(app, &directory);
    production_entries(
        "",
        [
            join_rel(&routes, &format!("*.{ROUTE_EXTENSIONS}")),
            join_rel(&routes, &format!("*/{{route,index}}.{ROUTE_EXTENSIONS}")),
        ]
        .iter()
        .map(String::as_str),
    )
}

/// The characters a glob would read as syntax in a file name written in `routes.ts` (`$`, `+` and parentheses are
/// plain characters in a glob).
fn escape_glob(file: &str) -> String {
    file.chars()
        .map(|c| match c {
            '[' | ']' | '{' | '}' | '*' | '?' => format!("[{c}]"),
            other => other.to_string(),
        })
        .collect()
}

/// The string literals of a source text that name a source file (`"routes/home.tsx"`).
fn quoted_files(source: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut quote = None;
    let mut current = String::new();
    let code = source
        .lines()
        .filter(|line| {
            !["//", "/*", "*"]
                .iter()
                .any(|comment| line.trim_start().starts_with(comment))
        })
        .collect::<Vec<_>>()
        .join("\n");
    for c in code.chars() {
        match quote {
            None if matches!(c, '"' | '\'' | '`') => quote = Some(c),
            Some(q) if c == q => {
                if looks_like_file(&current) && !current.contains("${") {
                    files.push(current.trim_start_matches("./").to_owned());
                }
                current.clear();
                quote = None;
            }
            Some(_) => current.push(c),
            None => {}
        }
    }
    files
}
