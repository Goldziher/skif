//! The JS analysis provider, wired to the plugin table of `skif-js-plugins`.

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::FileSystem;
use skif_js::{InputSource, JsAnalysis, JsProvider, PluginSource, UsageReport};
use skif_workspace::Workspace;

/// Builds the JS provider that reads through `fs` and takes its tool knowledge from the plugin table.
pub(crate) fn provider(config: &Config, fs: Arc<dyn FileSystem>) -> JsProvider {
    JsProvider::new(fs).with_input_source(Arc::new(
        PluginSource::new(config.js.plugins.clone())
            .with_workspaces(&config.js.workspaces)
            .with_overrides(&config.overrides),
    ))
}

/// Explains why the JavaScript analysis counts `file` (relative to the workspace root) as used, and with `export`
/// why that export of it is used or not.
///
/// The report names the entry rule when the file is an entry, lists every import of it with the importing line, and
/// shows the shortest chain of imports from an entry file; for an export it follows re-exports and ends in whether
/// another module uses it. It builds the whole analysis, so it costs as much as the JS part of a lint run.
#[must_use]
pub fn explain_usage(
    config: &Config,
    fs: &Arc<dyn FileSystem>,
    workspace: &Workspace,
    file: &Path,
    export: Option<&str>,
) -> UsageReport {
    let source: Arc<dyn InputSource> = Arc::new(
        PluginSource::new(config.js.plugins.clone())
            .with_workspaces(&config.js.workspaces)
            .with_overrides(&config.overrides),
    );
    JsAnalysis::build(workspace, config, fs, &[source], false).explain_usage(file, export)
}
