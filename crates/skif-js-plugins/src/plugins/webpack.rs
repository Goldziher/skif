//! webpack: entries and the loaders named in `module.rules`.

use skif_js_syntax::ConfigValue;

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::babel;
use crate::plugins::swc::plugin_packages;
use crate::support::{
    Entry, Spreads, alias_inputs, config_unknown, extension_inputs, files_key_unknown, get_in, html_script_sources,
    last_entry, local_script_path, looks_like_file, module_input, objects, strings,
};

/// The `webpack` plugin.
pub struct Webpack;

/// The plugin's identifier.
pub const ID: crate::plugin::PluginId = "webpack";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[Enabler::Dependency("webpack"), Enabler::Dependency("webpack-cli")],
    config: &["webpack.config.{js,ts,mjs,cjs,mts,cts}"],
    binaries: &["webpack", "webpack-cli"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Webpack {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        bundler_inputs(config, cx)
    }
}

/// What a webpack-compatible config (webpack, rspack) references: entries, loaders, aliases, lookup directories and
/// the scripts of the HTML templates.
pub(crate) fn bundler_inputs(config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
    let mut out = html_template_entries(cx);
    if config_unknown(config.value) {
        out.push(PluginInput::EntriesIncomplete);
    }
    for object in objects(config.value) {
        if let Some(entry) = object.get("entry") {
            entries(entry, config, &mut out);
        }
        if files_key_unknown(object, "entry") {
            out.push(PluginInput::EntriesIncomplete);
        }
        if module_unknown(object) {
            out.push(PluginInput::EntriesIncomplete);
        }
        if let Some(rules) = object.get("module").and_then(|m| m.get("rules")) {
            rule_loaders(rules, config, &mut out, 0);
            if rules_unknown(rules, 0) {
                out.push(PluginInput::EntriesIncomplete);
            }
        }
        if let Some(alias) = get_in(object, &["resolve", "alias"]) {
            out.extend(alias_inputs(alias, config));
        }
        if let Some(extensions) = get_in(object, &["resolve", "extensions"]) {
            out.extend(extension_inputs(extensions));
        }
        if let Some(modules) = get_in(object, &["resolve", "modules"]) {
            for directory in strings(modules) {
                if directory != "node_modules" && !directory.starts_with('/') {
                    out.push(PluginInput::ModuleDir(config.join(directory)));
                }
            }
        }
    }
    out
}

/// Scripts loaded by the conventional HTML templates (`public/index.html`, `src/index.html`, `templates/*.html`),
/// which `HtmlWebpackPlugin` and `HtmlRspackPlugin` inject the bundles into.
fn html_template_entries(cx: &PluginContext<'_>) -> Vec<PluginInput> {
    let patterns = ["index.html", "public/*.html", "src/*.html", "templates/*.html"].map(str::to_owned);
    let mut out = Vec::new();
    for html_path in find_files(cx.fs, cx.globs, cx.dir, &patterns) {
        let Ok(html) = cx.fs.read_to_string(&cx.dir.join(&html_path)) else {
            continue;
        };
        let html_dir = html_path.rsplit_once('/').map_or("", |(dir, _)| dir);
        for src in html_script_sources(&html) {
            if let Some(path) = local_script_path(&src, "", html_dir)
                && looks_like_file(&path)
            {
                out.push(PluginInput::Entry(path));
            }
        }
    }
    out
}

/// `entry` is a path, a list of paths, or an object whose values are either of those or `{ import }`; a value that is
/// one of several is each of them.
fn entries(entry: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    match entry {
        ConfigValue::String(path) => out.push(PluginInput::ProductionEntry(config.join(path))),
        ConfigValue::Array(items) | ConfigValue::OneOf(items) => {
            for item in items {
                entries(item, config, out);
            }
        }
        ConfigValue::Object(properties) => match entry.get("import") {
            Some(import) => entries(import, config, out),
            None => properties.iter().for_each(|(_, value)| entries(value, config, out)),
        },
        _ => {}
    }
}

fn rule_loaders(rules: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 6 {
        return;
    }
    for rule in objects(rules) {
        for key in ["use", "loader"] {
            for item in rule.get(key).map_or_default(items) {
                loader(item, rule.get("options"), config, out);
            }
        }
        for key in ["oneOf", "rules"] {
            if let Some(nested) = rule.get(key) {
                rule_loaders(nested, config, out, depth + 1);
            }
        }
    }
}

/// Whether the `module` of a config object may hold rules that are not known: it is unknown, or an object whose
/// `rules` an unknown spread may provide or replace. A `module` that is one of several objects counts as unknown, as
/// [`bundler_inputs`] reads the `rules` of a known object only.
fn module_unknown(object: &ConfigValue) -> bool {
    match last_entry(object, "module", Spreads::Any) {
        Some(Entry::Value(module)) => {
            module.is_unknown() || matches!(last_entry(module, "rules", Spreads::Any), Some(Entry::Spread))
        }
        Some(Entry::Spread) | None => false,
    }
}

/// Whether `rules` may hold a rule that is not known at all: an unknown element, a rule object that spreads an
/// unknown value, or a rule whose whole `use` or `loader` is unknown (`use: cssLoaders()`). A rule the config adds by
/// a change the evaluator does not follow is one of these, so the package is not judged without the loaders it names.
/// An unknown entry of a known `use` list (`MiniCssExtractPlugin.loader`) is not: its package is reached through the
/// import that names it. A `use` that is one of several known values is read as each ([`items`]).
fn rules_unknown(rules: &ConfigValue, depth: u8) -> bool {
    if depth > 6 {
        return true;
    }
    match rules {
        ConfigValue::Array(items) | ConfigValue::OneOf(items) => items.iter().any(|item| rules_unknown(item, depth)),
        ConfigValue::Object(_) => {
            rules.get("...").is_some()
                || ["use", "loader"]
                    .iter()
                    .any(|key| matches!(rules.get(key), Some(ConfigValue::Unknown)))
                || ["oneOf", "rules"]
                    .iter()
                    .any(|key| rules.get(key).is_some_and(|nested| rules_unknown(nested, depth + 1)))
        }
        other => other.is_unknown(),
    }
}

/// A loader is a string or `{ loader, options }`; the rule's own `options` apply to a string `loader`.
fn loader(item: &ConfigValue, rule_options: Option<&ConfigValue>, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>) {
    let (name, options) = match item {
        ConfigValue::String(name) => (name.as_str(), rule_options),
        ConfigValue::Object(_) => match item.get("loader").and_then(ConfigValue::as_str) {
            Some(name) => (name, item.get("options")),
            None => return,
        },
        _ => return,
    };
    out.extend(loader_input(config, name));
    if is_swc_loader(name)
        && matches!(item, ConfigValue::Object(_))
        && let Some(options) = item.get("options")
    {
        out.extend(
            plugin_packages(options)
                .into_iter()
                .map(PluginInput::OptionalDependency),
        );
    }
    if name.split('?').next() == Some("babel-loader")
        && let Some(options) = options
    {
        out.extend(babel::dependencies(options, config.base, 0));
    }
}

fn is_swc_loader(name: &str) -> bool {
    name == "builtin:swc-loader" || name == "swc-loader" || name.ends_with("/swc-loader")
}

fn loader_input(config: &ConfigAst<'_>, name: &str) -> Option<PluginInput> {
    module_input(config.base, name.split('?').next().unwrap_or_default(), false)
}

/// The loaders of a `use` or `loader` value: a list, a single loader, or each alternative of a value that is one of
/// several.
fn items(value: &ConfigValue) -> Vec<&ConfigValue> {
    match value {
        ConfigValue::Array(items) => items.iter().collect(),
        ConfigValue::OneOf(alternatives) => alternatives.iter().flat_map(items).collect(),
        other => vec![other],
    }
}
