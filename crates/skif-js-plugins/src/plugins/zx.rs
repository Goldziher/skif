//! zx: the commands of `` $`…` `` in a zx script (a file starting with `#!/usr/bin/env zx`, where `$` is a global) or in a
//! module that imports `$` from `zx` are scripts, parsed like the scripts of `package.json`.

use skif_js_syntax::extract_imported_calls;

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::sources_containing;

/// The `zx` plugin.
pub struct Zx;

static SPEC: PluginSpec = PluginSpec {
    id: "zx",
    enablers: &[Enabler::Dependency("zx")],
    ..PluginSpec::DEFAULT
};

impl Plugin for Zx {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for (source, text) in sources_containing(cx, "$`") {
            let path = cx.dir.join(&source);
            let globals: &[(&str, &str)] = if text.starts_with("#!/usr/bin/env zx") {
                &[("$", "zx")]
            } else {
                &[]
            };
            for call in extract_imported_calls(&text, &path, &["zx"], globals) {
                if call.export == "$" {
                    out.extend(call.template.map(PluginInput::Script));
                }
            }
        }
        out
    }
}
