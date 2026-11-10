//! Vite: HTML entry points, build inputs and pre-bundled dependencies.

use skif_js_syntax::ConfigValue;

use crate::config_load::load_config;
use crate::files::find_files;
use crate::plugin::{
    ConfigAst, Enabler, IncompleteCause, IncompleteScope, Plugin, PluginContext, PluginInput, PluginSpec,
};
use crate::plugins::postcss;
use crate::support::{
    Spreads, alias_inputs, convention_scope, extension_inputs, get_in, html_entries, html_entries_at, join_rel,
    package_name, path_unknown, strings, values_of,
};

/// The keys Vite takes entry files from: the project root holding `index.html`, the build inputs and the library
/// entries.
const ENTRY_KEYS: &[&[&str]] = &[&["root"], BUILD_INPUTS[0], BUILD_INPUTS[1], &["build", "lib", "entry"]];

/// Where a config sets the build input: Vite 8 bundles with Rolldown and reads `build.rolldownOptions`, earlier
/// versions `build.rollupOptions`, which Vite 8 still takes as an alias.
pub(crate) const BUILD_INPUTS: [&[&str]; 2] = [
    &["build", "rolldownOptions", "input"],
    &["build", "rollupOptions", "input"],
];

/// The key of [`BUILD_INPUTS`] that `value` (a Vite config, or a section of one) takes its build input from: the
/// Rolldown options when it sets them, else the Rollup options, as knip reads them.
pub(crate) fn build_input_key(value: &ConfigValue) -> &'static [&'static str] {
    if get_in(value, BUILD_INPUTS[0]).is_some() {
        BUILD_INPUTS[0]
    } else {
        BUILD_INPUTS[1]
    }
}

/// The `vite` plugin.
pub struct Vite;

static SPEC: PluginSpec = PluginSpec {
    id: "vite",
    enablers: &[
        Enabler::Dependency("vite"),
        Enabler::Dependency("vitest"),
        Enabler::Dependency("vite-plus"),
    ],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    binaries: &["vite"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Vite {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = html_and_build_inputs(config, cx);
        if path_unknown(config.value, build_input_key(config.value), Spreads::Any)
            || path_unknown(config.value, &["build", "lib", "entry"], Spreads::Any)
        {
            out.push(PluginInput::EntriesIncomplete);
        }
        if let Some(entry) = get_in(config.value, &["build", "lib", "entry"]) {
            for path in values_of(entry).into_iter().flat_map(strings) {
                out.push(PluginInput::ProductionEntry(config.join(path)));
            }
        }
        if let Some(include) = get_in(config.value, &["optimizeDeps", "include"]) {
            for specifier in strings(include) {
                let id = specifier.split('>').next().unwrap_or_default().trim();
                // Pre-bundling a package does not need it installed; knip reads it as an optional reference.
                out.extend(package_name(id).map(PluginInput::OptionalDependency));
            }
        }
        if let Some(alias) = get_in(config.value, &["resolve", "alias"]) {
            out.extend(alias_inputs(alias, config));
        }
        if let Some(extensions) = get_in(config.value, &["resolve", "extensions"]) {
            out.extend(extension_inputs(extensions));
        }
        if let Some(postcss) = get_in(config.value, &["css", "postcss"]) {
            out.extend(css_postcss(postcss, config, cx));
        }
        out
    }

    fn resolve_without_config(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        html_entries(cx, "", "index.html")
    }

    /// A merged or spread imported base config is shared between packages, so it names no file of this one; at most
    /// it replaces the build input with the conventional `index` or `main` module. An entry key the config computes
    /// itself (`entry: pick()`, a spread inside it, or a spread above it of a value the config computes,
    /// `build: { ...makeLib() }`), or a config that is unknown as a whole, may name any file.
    fn incomplete_scope(&self, _cause: IncompleteCause, config: &ConfigAst<'_>) -> IncompleteScope {
        if ENTRY_KEYS
            .iter()
            .any(|path| path_unknown(config.value, path, Spreads::Own))
        {
            return IncompleteScope::Package;
        }
        convention_scope(&SPEC, config, &["{,src/}{index,main}.{ext}"])
    }
}

/// The `index.html` scripts and the build input entries ([`build_input_key`]) of a config; the vitest plugin reuses
/// them the way knip does.
///
/// `root` moves the project root, where Vite looks for `index.html` and resolves `/`-rooted paths (the `src` of a
/// script tag, an HTML input written as `/page.html`), relative to the config file. A `root` that is not a static
/// string leaves those entries unknown.
pub(crate) fn html_and_build_inputs(config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
    let mut out = Vec::new();
    let root = match get_in(config.value, &["root"]) {
        None => config.base.to_owned(),
        Some(ConfigValue::String(root)) if !std::path::Path::new(root).is_absolute() => config.join(root),
        Some(_) => {
            out.push(PluginInput::EntriesIncomplete);
            config.base.to_owned()
        }
    };
    out.extend(html_entries(cx, &root, "index.html"));
    let inputs = get_in(config.value, build_input_key(config.value));
    for input in inputs.into_iter().flat_map(values_of).flat_map(strings) {
        if std::path::Path::new(input)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
        {
            out.extend(match input.strip_prefix('/') {
                Some(rooted) => html_entries(cx, &root, rooted),
                None => html_entries_at(cx, &root, &config.join(input)),
            });
        } else {
            out.push(PluginInput::ProductionEntry(config.join(input)));
        }
    }
    out
}

/// `css.postcss`: an inline `PostCSS` config object, or the path of a `PostCSS` config file or of the directory to
/// search for one. A config file is an entry, and what it references counts for this plugin.
fn css_postcss(value: &ConfigValue, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
    let ConfigValue::String(path) = value else {
        return postcss::inputs(value, config.base);
    };
    let target = config.join(path);
    let file = if cx.fs.is_file(&cx.dir.join(&target)) {
        Some(target)
    } else {
        let patterns: Vec<String> = postcss::CONFIG.iter().map(|pattern| (*pattern).to_owned()).collect();
        find_files(cx.fs, &cx.dir.join(&target), &patterns)
            .into_iter()
            .next()
            .map(|found| join_rel(&target, &found))
    };
    let Some(file) = file else {
        return Vec::new();
    };
    let Some(postcss_config) = cx
        .fs
        .read_to_string(&cx.dir.join(&file))
        .ok()
        .and_then(|source| load_config(&cx.dir.join(&file), &source))
    else {
        return vec![PluginInput::Entry(file)];
    };
    let base = file.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut out = postcss::inputs(&postcss_config, base);
    out.push(PluginInput::Entry(file));
    out
}
