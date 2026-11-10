//! zx: the commands of `` $`…` `` in a zx script (a file whose shebang runs `zx`, such as `#!/usr/bin/env zx` or
//! `#!/usr/bin/env -S npx zx`, or that imports `zx/globals`, where `$` is a global) or in a module that imports `$` from
//! `zx` are scripts, parsed like the scripts of `package.json`.

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
            let globals: &[(&str, &str)] = if has_globals(&text) { &[("$", "zx")] } else { &[] };
            for call in extract_imported_calls(&text, &path, &["zx"], globals) {
                if call.export == "$" {
                    out.extend(call.template.map(|script| PluginInput::SourceScript {
                        file: source.clone(),
                        script,
                    }));
                }
            }
        }
        out
    }
}

/// Whether `$` is a global in `text`: its shebang runs `zx`, or it imports `zx/globals`.
fn has_globals(text: &str) -> bool {
    let shebang = text.lines().next().filter(|line| line.starts_with("#!"));
    shebang.is_some_and(|line| {
        line.split_whitespace()
            .any(|word| word == "zx" || word.ends_with("/zx"))
    }) || text.lines().map(str::trim_start).any(|line| {
        (line.starts_with("import") || line.contains("require("))
            && (line.contains("'zx/globals'") || line.contains("\"zx/globals\""))
    })
}
