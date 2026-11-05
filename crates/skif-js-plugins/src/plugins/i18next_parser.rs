//! `i18next-parser`: the config file is an entry file.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `i18next-parser` plugin.
pub struct I18nextParser;

static SPEC: PluginSpec = PluginSpec {
    id: "i18next-parser",
    enablers: &[Enabler::Dependency("i18next-parser")],
    entry: &["i18next-parser.config.{js,mjs,json,ts,yaml,yml}"],
    binaries: &["i18next"],
    ..PluginSpec::DEFAULT
};

impl Plugin for I18nextParser {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
