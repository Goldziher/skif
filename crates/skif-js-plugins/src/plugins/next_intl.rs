//! next-intl: the request config module the plugin of `next.config` loads.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `next-intl` plugin.
pub struct NextIntl;

static SPEC: PluginSpec = PluginSpec {
    id: "next-intl",
    enablers: &[Enabler::Dependency("next-intl")],
    production: &["{,src/}i18n/request.{js,jsx,ts,tsx}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for NextIntl {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
