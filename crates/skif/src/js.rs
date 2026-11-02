//! The JS analysis provider, wired to the plugin table of `skif-js-plugins`.

use std::sync::Arc;

use skif_config::Config;
use skif_core::FileSystem;
use skif_js::{JsProvider, PluginSource};

/// Builds the JS provider that reads through `fs` and takes its tool knowledge from the plugin table.
pub(crate) fn provider(config: &Config, fs: Arc<dyn FileSystem>) -> JsProvider {
    JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())))
}
