//! playwright-test: the first file or glob a script passes to `playwright-test` is an entry; a config passed with
//! `--config` is a config file.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{positionals, workspace_path};
use crate::support::{extglob_to_braces, join_rel};

/// The `playwright-test` plugin.
pub struct PlaywrightTest;

static SPEC: PluginSpec = PluginSpec {
    id: "playwright-test",
    enablers: &[Enabler::Dependency("playwright-test")],
    binaries: &["playwright-test", "pw-test"],
    ..PluginSpec::DEFAULT
};

impl Plugin for PlaywrightTest {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        cx.scripts
            .values()
            .flat_map(|script| positionals(script, SPEC.binaries, &["quiet", "verbose", "watch"]))
            .filter_map(|words| words.into_iter().find(|word| word != "install" && word != "test"))
            .filter_map(|word| {
                if word.contains(['*', '{']) {
                    let glob = join_rel("", &word);
                    (!glob.starts_with('/') && !glob.starts_with("..") && !glob.contains('$')).then_some(glob)
                } else {
                    workspace_path(&word)
                }
            })
            .map(|path| PluginInput::Entry(extglob_to_braces(&path)))
            .collect()
    }
}
