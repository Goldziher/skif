//! Vite: HTML entry points, build inputs and pre-bundled dependencies.

use skif_js_syntax::ConfigValue;

use crate::config_load::load_config;
use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::postcss;
use crate::support::{
    alias_inputs, extension_inputs, get_in, html_entries, join_rel, package_name, path_unknown, strings, values_of,
};

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
        if path_unknown(config.value, &["build", "rollupOptions", "input"])
            || path_unknown(config.value, &["build", "lib", "entry"])
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
                out.extend(package_name(id).map(PluginInput::Dependency));
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
}

/// The `index.html` scripts and the `build.rollupOptions.input` entries of a config; the vitest plugin reuses them
/// the way knip does.
pub(crate) fn html_and_build_inputs(config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
    let mut out = html_entries(cx, config.base, "index.html");
    let inputs = get_in(config.value, &["build", "rollupOptions", "input"]);
    for input in inputs.into_iter().flat_map(values_of).flat_map(strings) {
        if std::path::Path::new(input)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
        {
            out.extend(html_entries(cx, config.base, input.trim_start_matches('/')));
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
