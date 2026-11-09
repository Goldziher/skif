//! Eleventy: the data files and JavaScript templates under the input directory `eleventy.config.*` returns
//! (`dir.input`, `dir.data`, `templateFormats`), and the passthrough copies of `addPassthroughCopy("img")` and
//! `addPassthroughCopy({ "src/js": "js" })`, as knip reads them: a directory copies its JavaScript and TypeScript
//! files, which are production entries, a file or glob is one itself, and a path into `node_modules` names a package.

use skif_js_syntax::{ConfigValue, extract_method_calls};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, join_rel, package_name, strings};

/// The `eleventy` plugin.
pub struct Eleventy;

static SPEC: PluginSpec = PluginSpec {
    id: "eleventy",
    enablers: &[Enabler::Dependency("@11ty/eleventy")],
    config: &[".eleventy.js", "eleventy.config.{js,cjs,mjs}"],
    production: &["posts/**/*.11tydata.{js,cjs,mjs}", "_data/**/*.{js,cjs,mjs}"],
    binaries: &["eleventy"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Eleventy {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let dir = |key: &str, default: &'static str| {
            get_in(config.value, &["dir", key])
                .and_then(ConfigValue::as_str)
                .filter(|dir| !dir.is_empty())
                .unwrap_or(default)
        };
        let input = dir("input", ".");
        let data = dir("data", "_data");
        let formats = match config.value.get("templateFormats") {
            Some(formats @ ConfigValue::Array(_)) => strings(formats).join(","),
            Some(ConfigValue::String(formats)) => formats.clone(),
            _ => "11ty.js".to_owned(),
        };
        let mut out: Vec<PluginInput> = [
            join_rel(&join_rel(input, data), "**/*.{js,cjs,mjs}"),
            join_rel(input, &format!("**/*.{{{formats}}}")),
            join_rel(input, "**/*.11tydata.{js,cjs,mjs}"),
        ]
        .into_iter()
        .map(|pattern| PluginInput::ProductionEntry(config.join(&pattern)))
        .collect();
        for arguments in extract_method_calls(config.source, config.path, "addPassthroughCopy") {
            let paths: Vec<&str> = match arguments.first() {
                Some(ConfigValue::String(path)) => vec![path.as_str()],
                Some(ConfigValue::Object(entries)) => entries
                    .iter()
                    .map(|(path, _)| path.as_str())
                    .filter(|path| *path != "...")
                    .collect(),
                _ => Vec::new(),
            };
            out.extend(paths.into_iter().filter_map(|path| passthrough_copy(path, config, cx)));
        }
        out
    }
}

/// The extensions of the copied files that are modules, as knip lists them.
const COPIED_EXTENSIONS: &str = "{js,mjs,cjs,jsx,ts,tsx,mts,cts}";

/// What a passthrough copy path names: the modules of a directory, a file or glob, or a package in `node_modules`.
fn passthrough_copy(path: &str, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Option<PluginInput> {
    let path = path.trim();
    if path.is_empty() || path.starts_with('/') {
        return None;
    }
    if let Some((_, installed)) = path.split_once("node_modules/") {
        return package_name(installed).map(PluginInput::Dependency);
    }
    let file = config.join(path);
    if !path.contains('*') && cx.fs.is_dir(&cx.dir.join(&file)) {
        return Some(PluginInput::ProductionEntry(join_rel(
            &file,
            &format!("**/*.{COPIED_EXTENSIONS}"),
        )));
    }
    Some(PluginInput::ProductionEntry(file))
}
