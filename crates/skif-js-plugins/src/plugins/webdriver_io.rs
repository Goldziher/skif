//! `WebdriverIO`: the framework, runner and reporters the config names are `@wdio` packages.

use std::path::Path;

use skif_js_syntax::{ConfigValue, extract_config_object};

use crate::config_load::load_config;
use crate::files::find_files;
use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{name_of, values_of};

/// The `webdriver-io` plugin.
pub struct WebdriverIo;

const CONFIG: &str = "wdio.conf.{js,cjs,mjs,ts,cts,mts}";

static SPEC: PluginSpec = PluginSpec {
    id: "webdriver-io",
    enablers: &[Enabler::Dependency("@wdio/cli")],
    config: &[CONFIG],
    binaries: &["wdio"],
    ..PluginSpec::DEFAULT
};

impl Plugin for WebdriverIo {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for file in find_files(cx.fs, cx.globs, cx.dir, &[CONFIG.to_owned()]) {
            let path = cx.dir.join(&file);
            let Ok(source) = cx.fs.read_to_string(&path) else {
                continue;
            };
            if let Some(options) = options(&path, &source) {
                out.extend(packages(&options));
            }
        }
        out
    }
}

/// The `config` object: the default export's `config` key, or the named export `config` that `WebdriverIO` configs
/// declare (`export const config = {…}`, `exports.config = {…}`).
fn options(path: &Path, source: &str) -> Option<ConfigValue> {
    if let Some(config) = load_config(path, source).and_then(|value| value.get("config").cloned()) {
        return Some(config);
    }
    let rewritten = ["export const config", "exports.config"].iter().find_map(|marker| {
        let start = source.find(marker)?;
        let rest = &source[start + marker.len()..];
        let assign = rest.find('=')?;
        let head = if marker.starts_with("exports") {
            "module.exports ="
        } else {
            "export default"
        };
        Some(format!("{}{head} {}", &source[..start], &rest[assign + 1..]))
    })?;
    extract_config_object(&rewritten, path)
}

fn packages(options: &ConfigValue) -> Vec<PluginInput> {
    let mut names = Vec::new();
    if let Some(framework) = options.get("framework").and_then(ConfigValue::as_str) {
        names.push(format!("@wdio/{framework}-framework"));
    }
    if let Some(runner) = options.get("runner").and_then(name_of)
        && runner != "local"
    {
        names.push(format!("@wdio/{runner}-runner"));
    }
    if let Some(reporters) = options.get("reporters") {
        names.extend(
            values_of(reporters)
                .into_iter()
                .filter_map(name_of)
                .map(|reporter| format!("@wdio/{reporter}-reporter")),
        );
    }
    names.into_iter().map(PluginInput::Dependency).collect()
}
