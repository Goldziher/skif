//! `@nuxtjs/i18n`: the Vue I18n config file the module loads is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `nuxtjs-i18n` plugin.
pub struct NuxtjsI18n;

static SPEC: PluginSpec = PluginSpec {
    id: "nuxtjs-i18n",
    enablers: &[Enabler::Dependency("@nuxtjs/i18n")],
    entry: &["i18n.config.{js,mjs,ts}", "i18n/i18n.config.{js,mjs,ts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for NuxtjsI18n {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
