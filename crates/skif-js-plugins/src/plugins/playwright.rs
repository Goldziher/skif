//! Playwright: test files, global setup, reporters and the web server command named in the config.

use skif_js_syntax::ConfigValue;

use crate::plugin::{
    ConfigAst, Enabler, IncompleteCause, IncompleteScope, Plugin, PluginContext, PluginInput, PluginSpec,
};
use crate::plugins::vitest::TEST_CONVENTIONS;
use crate::support::{
    Named, Spreads, config_unknown, convention_scope, entry_glob, join_rel, module_input, objects, reporter_names,
    reporters_unknown, strings,
};

/// The `playwright` plugin.
pub struct Playwright;

pub(crate) const TEST_FILES: &str = "**/*.{spec,test}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}";

/// The plugin's identifier.
pub const ID: crate::plugin::PluginId = "playwright";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[Enabler::Dependency("@playwright/test")],
    config: &["playwright.config.{js,cjs,mjs,ts,cts,mts}"],
    entry: &[TEST_FILES],
    binaries: &["playwright"],
    optional_binaries: true,
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
            for path in config.value.get(key).map_or_default(strings) {
                out.push(PluginInput::Entry(config.join(path)));
            }
        }
        for named in config.value.get("reporter").map_or_default(reporter_names) {
            if let Named::Known(name) = named
                && !BUILTIN_REPORTERS.contains(&name)
            {
                out.extend(module_input(config.base, name, true));
            }
        }
        if computes_reporters(config.value) {
            out.push(PluginInput::EntriesIncomplete);
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

    /// A Playwright config names test files, test directories, setup and teardown modules and reporter modules; an
    /// imported base config it spreads or merges is taken to hide only the tests and setup modules, where the tool
    /// looks by convention. A config unknown as a whole (a factory called with this package's options) and a reporter
    /// list the config computes itself may name any file.
    fn incomplete_scope(&self, _cause: IncompleteCause, config: &ConfigAst<'_>) -> IncompleteScope {
        if config_unknown(config.value) || computes_reporters(config.value) {
            return IncompleteScope::Package;
        }
        convention_scope(&SPEC, config, TEST_CONVENTIONS)
    }
}

/// Whether the config computes its `reporter` list in a way that hides which modules it names: a reporter name is not
/// known statically, every branch of a condition counting ([`reporter_names`]), or a value the config computes itself
/// and spreads after the key may set it. An imported base config that may set it is an unknown spread of the whole
/// config, which leaves the entries incomplete already.
fn computes_reporters(config: &ConfigValue) -> bool {
    reporters_unknown(config, "reporter", Spreads::Own)
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
        ConfigValue::Regex { pattern: source, .. } => vec![regex_glob(source).unwrap_or_else(|| ANY_SCRIPT.to_owned())],
        _ => Vec::new(),
    }
}

/// A glob matching at least the files a regular expression of file-name fragments matches: `.*\.setup\.js` becomes
/// `**/*.setup.js*` and `.*\.e2e\.tsx?$` becomes `**/*.e2e.ts{x,}`. `None` when the expression uses more than
/// literals, `.*`, character classes (`[jt]`), the `?` quantifier, anchors and alternation groups.
fn regex_glob(source: &str) -> Option<String> {
    let anchored_end = source.ends_with('$') && !source.ends_with("\\$");
    let body = source.strip_prefix('^').unwrap_or(source);
    let body = if anchored_end { body.strip_suffix('$')? } else { body };
    let mut glob = String::new();
    // Where the last atom (a character, a class or a group) starts in `glob`, for a `?` that follows it; `None` after
    // `.*` and at the start, where `?` is lazy or has nothing to repeat.
    let mut atom: Option<usize> = None;
    let mut groups: Vec<usize> = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let start = glob.len();
        match c {
            '\\' => {
                let escaped = chars.next()?;
                if escaped.is_ascii_alphanumeric() {
                    return None;
                }
                glob.push(escaped);
                atom = Some(start);
            }
            '.' if chars.peek() == Some(&'*') => {
                chars.next();
                glob.push('*');
                atom = None;
            }
            '[' => {
                glob.push_str(&class_glob(&mut chars)?);
                atom = Some(start);
            }
            '?' => match atom.take() {
                Some(optional) => {
                    let repeated = glob.split_off(optional);
                    glob.push('{');
                    glob.push_str(&repeated);
                    glob.push_str(",}");
                }
                // `.*?` is lazy and matches what `.*` does.
                None if glob.ends_with('*') => {}
                None => return None,
            },
            '(' => {
                if chars.peek() == Some(&'?') {
                    chars.next();
                    if chars.next()? != ':' {
                        return None;
                    }
                }
                groups.push(start);
                glob.push('{');
                atom = None;
            }
            ')' => {
                atom = Some(groups.pop()?);
                glob.push('}');
            }
            '|' if !groups.is_empty() => {
                glob.push(',');
                atom = None;
            }
            c if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/') => {
                glob.push(c);
                atom = Some(start);
            }
            _ => return None,
        }
    }
    if !groups.is_empty() || glob.is_empty() {
        return None;
    }
    let tail = if anchored_end { "" } else { "*" };
    let head = if glob.starts_with('*') { "**/" } else { "**/*" };
    Some(format!("{head}{glob}{tail}"))
}

/// The glob class for a regular expression class whose `[` has been read: `[jt]` stays `[jt]` and `[^.]` becomes
/// `[!.]`. `None` for an empty or unterminated class and for anything but letters, digits, `-`, `_`, `.` and escaped
/// punctuation.
fn class_glob(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut class = String::from("[");
    if chars.peek() == Some(&'^') {
        chars.next();
        class.push('!');
    }
    let mut members = 0_usize;
    loop {
        match chars.next()? {
            ']' if members > 0 => break,
            '\\' => {
                let escaped = chars.next()?;
                if escaped.is_ascii_alphanumeric() || matches!(escaped, '[' | ']' | '\\' | '/' | '!' | '^') {
                    return None;
                }
                class.push(escaped);
            }
            c if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') => class.push(c),
            _ => return None,
        }
        members += 1;
    }
    class.push(']');
    Some(class)
}
