//! `@intlify/unplugin-vue-i18n`: enabled by its package and, as in knip, without entries or configs of its own; its
//! `@intlify/unplugin-vue-i18n/messages` module is imported from the package it lists.

use crate::plugin::{Enabler, PORTED_KNIP_COMMIT, Plugin, PluginSpec};

/// The `unplugin-vue-i18n` plugin.
pub struct UnpluginVueI18n;

static SPEC: PluginSpec = PluginSpec {
    id: "unplugin-vue-i18n",
    enablers: &[Enabler::Dependency("@intlify/unplugin-vue-i18n")],
    knip_commit: PORTED_KNIP_COMMIT,
    ..PluginSpec::DEFAULT
};

impl Plugin for UnpluginVueI18n {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
