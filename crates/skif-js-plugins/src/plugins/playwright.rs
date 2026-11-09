//! Playwright: test files, global setup, reporters and the web server command named in the config.

use skif_js_syntax::ConfigValue;

use crate::plugin::{
    ConfigAst, Enabler, IncompleteCause, IncompleteScope, Plugin, PluginContext, PluginInput, PluginSpec,
};
use crate::plugins::vitest::TEST_CONVENTIONS;
use crate::support::{config_unknown, convention_scope, entry_glob, join_rel, module_input, name_of, objects, strings};

/// The `playwright` plugin.
pub struct Playwright;

pub(crate) const TEST_FILES: &str = "**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}";

static SPEC: PluginSpec = PluginSpec {
    id: "playwright",
    enablers: &[Enabler::Dependency("@playwright/test")],
    config: &["playwright.config.{js,cjs,mjs,ts,cts,mts}"],
    entry: &[TEST_FILES],
    binaries: &["playwright"],
    ..PluginSpec::DEFAULT
};

const BUILTIN_REPORTERS: &[&str] = &["dot", "line", "list", "junit", "html", "blob", "json", "github", "null"];

impl Plugin for Playwright {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let root_dir = config.value.get("testDir").and_then(ConfigValue::as_str);
        let mut projects = vec![config.value];
        if let Some(listed) = config.value.get("projects") {
            projects.extend(objects(listed));
        }
        for project in projects {
            test_files(project, root_dir, config, &mut out);
        }
        for key in ["globalSetup", "globalTeardown"] {
            for path in config.value.get(key).map(strings).unwrap_or_default() {
                out.push(PluginInput::Entry(config.join(path)));
            }
        }
        if let Some(reporter) = config.value.get("reporter") {
            let reporters: Vec<&ConfigValue> = match reporter {
                ConfigValue::Array(items) => items.iter().collect(),
                other => vec![other],
            };
            for name in reporters.into_iter().filter_map(name_of) {
                if !BUILTIN_REPORTERS.contains(&name) {
                    out.extend(module_input(config.base, name, true));
                }
            }
        }
        if let Some(servers) = config.value.get("webServer") {
            for server in objects(servers) {
                if let Some(command) = server.get("command").and_then(ConfigValue::as_str) {
                    out.push(PluginInput::Script(command.to_string()));
                }
            }
        }
        out
    }

    /// A Playwright config names test files, test directories and setup and teardown modules, nothing else, so a
    /// spread or merged base hides only those. A config unknown as a whole (a factory called with this package's
    /// options) may name any file.
    fn incomplete_scope(&self, _cause: IncompleteCause, config: &ConfigAst<'_>) -> IncompleteScope {
        if config_unknown(config.value) {
            return IncompleteScope::Package;
        }
        convention_scope(&SPEC, config, TEST_CONVENTIONS)
    }
}

/// Test globs of one project: `testMatch` under its `testDir`, or every test file under a custom `testDir`.
fn test_files(project: &ConfigValue, root_dir: Option<&str>, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    let dir = project.get("testDir").and_then(ConfigValue::as_str).or(root_dir);
    let base = dir.map_or_else(|| config.base.to_string(), |d| join_rel(config.base, d));
    let patterns: Vec<String> = match project.get("testMatch") {
        Some(ConfigValue::Array(items)) => items.iter().flat_map(match_patterns).collect(),
        Some(single) => match_patterns(single),
        None => Vec::new(),
    };
    if !patterns.is_empty() {
        for pattern in patterns {
            out.extend(entry_glob(&base, &pattern).map(PluginInput::Entry));
        }
    } else if dir.is_some() {
        out.push(PluginInput::Entry(join_rel(&base, TEST_FILES)));
    }
}

/// Every script file, which is what a `testMatch` regular expression that is not a simple file name pattern may match.
const ANY_SCRIPT: &str = "**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}";

/// The globs for one `testMatch` value: a glob string, or a regular expression turned into a glob.
fn match_patterns(value: &ConfigValue) -> Vec<String> {
    match value {
        ConfigValue::String(glob) => vec![glob.clone()],
        ConfigValue::Regex(source) => vec![regex_glob(source).unwrap_or_else(|| ANY_SCRIPT.to_owned())],
        _ => Vec::new(),
    }
}

/// A glob matching at least the files a regular expression of file-name fragments matches: `.*\.setup\.js` becomes
/// `**/*.setup.js*` and `.*\.e2e\.tsx?` becomes `**/*.e2e.ts{x,}*`. `None` when the expression uses more than
/// literals, `.*`, `?` after a literal character, anchors and alternation groups.
fn regex_glob(source: &str) -> Option<String> {
    let anchored_end = source.ends_with('$') && !source.ends_with("\\$");
    let body = source.strip_prefix('^').unwrap_or(source);
    let body = if anchored_end { body.strip_suffix('$')? } else { body };
    let mut glob = String::new();
    let mut depth = 0_u32;
    // The literal character just written, which a following `?` makes optional.
    let mut last_literal: Option<char> = None;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let literal = match c {
            '\\' => {
                let escaped = chars.next()?;
                if escaped.is_ascii_alphanumeric() || matches!(escaped, '*' | '?' | '[' | ']' | '{' | '}' | ',') {
                    return None;
                }
                glob.push(escaped);
                Some(escaped)
            }
            '?' => {
                let optional = last_literal.take()?;
                glob.pop();
                glob.push('{');
                glob.push(optional);
                glob.push_str(",}");
                None
            }
            '.' if chars.peek() == Some(&'*') => {
                chars.next();
                glob.push('*');
                None
            }
            '(' => {
                if chars.peek() == Some(&'?') {
                    chars.next();
                    if chars.next()? != ':' {
                        return None;
                    }
                }
                depth += 1;
                glob.push('{');
                None
            }
            ')' => {
                depth = depth.checked_sub(1)?;
                glob.push('}');
                None
            }
            '|' if depth > 0 => {
                glob.push(',');
                None
            }
            c if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/') => {
                glob.push(c);
                Some(c)
            }
            _ => return None,
        };
        last_literal = literal;
    }
    if depth != 0 || glob.is_empty() {
        return None;
    }
    let tail = if anchored_end { "" } else { "*" };
    let head = if glob.starts_with('*') { "**/" } else { "**/*" };
    Some(format!("{head}{glob}{tail}"))
}
