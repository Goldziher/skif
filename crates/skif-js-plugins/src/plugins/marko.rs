//! Marko: the tag files under `components/` and `tags/` (next to a `marko.json`, or anywhere) are production entries,
//! the modules a `marko-tag.json` names are used, and `.marko` files use the `marko` package.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, module_input, strings};

/// The `marko` plugin.
pub struct Marko;

/// The tag files Marko discovers, relative to a directory that holds `components/` or `tags/`.
const TAG_FILES: &[&str] = &[
    "**/{components,tags}/**/*.marko",
    "**/{components,tags}/**/{component,component-browser}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
    "**/{components,tags}/**/*.{component,component-browser}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
    "**/{components,tags}/**/style.{css,less,scss,sass,styl,stylus}",
    "**/{components,tags}/**/*.style.{css,less,scss,sass,styl,stylus}",
];

/// The fields of a tag definition that name a module.
const TAG_FIELDS: &[&str] = &[
    "template",
    "renderer",
    "parse",
    "migrate",
    "transform",
    "analyze",
    "translate",
];

static SPEC: PluginSpec = PluginSpec {
    id: "marko",
    enablers: &[Enabler::Dependency("marko")],
    config: &["**/marko.json", "**/marko-tag.json"],
    production: TAG_FILES,
    ..PluginSpec::DEFAULT
};

impl Plugin for Marko {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        if config.file_name() == "marko.json" {
            return TAG_FILES
                .iter()
                .map(|pattern| PluginInput::ProductionEntry(join_rel(config.base, pattern)))
                .collect();
        }
        TAG_FIELDS
            .iter()
            .filter_map(|field| config.value.get(field))
            .flat_map(strings)
            .filter_map(|id| module_input(config.base, id, true))
            .collect()
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let templates = crate::files::find_files(cx.fs, cx.globs, cx.dir, &["**/*.marko".to_owned()]);
        let has_templates = !templates.is_empty();
        has_templates
            .then(|| PluginInput::Dependency("marko".to_owned()))
            .into_iter()
            .collect()
    }
}
