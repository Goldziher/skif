//! unplugin-vue-markdown: enabled by its package, without entries or configs of its own. knip compiles the Markdown
//! files it turns into Vue components to find the components their tags use; skif reads no Markdown, so where the
//! `unplugin-vue-components` options of a bundler config `include` Markdown files, every component the declaration
//! file registers counts as used (ADR 0020).

use crate::plugin::{Enabler, PORTED_KNIP_COMMIT, Plugin, PluginSpec};

/// The `unplugin-vue-markdown` plugin.
pub struct UnpluginVueMarkdown;

static SPEC: PluginSpec = PluginSpec {
    id: "unplugin-vue-markdown",
    enablers: &[Enabler::Dependency("unplugin-vue-markdown")],
    knip_commit: PORTED_KNIP_COMMIT,
    ..PluginSpec::DEFAULT
};

impl Plugin for UnpluginVueMarkdown {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
