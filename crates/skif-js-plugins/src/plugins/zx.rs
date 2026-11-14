//! zx: the commands of `` $`…` `` in a zx script (a file whose shebang runs `zx`, such as `#!/usr/bin/env zx` or
//! `#!/usr/bin/env -S npx zx`, or that imports `zx/globals`, where `$` is a global) or in a module that imports `$` from
//! `zx` are scripts, parsed like the scripts of `package.json`. In a file that switches zx to PowerShell (`usePwsh()`,
//! `usePowerShell()`, `$.shell = 'pwsh'`), the cmdlets and default aliases PowerShell runs itself are no binaries.

use skif_js_syntax::extract_imported_calls;

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::scripts::{is_powershell_command, without_commands};
use crate::support::own_sources_containing;

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
        for (source, text) in own_sources_containing(cx, "$`") {
            let path = cx.dir.join(&source);
            let globals: &[(&str, &str)] = if has_globals(&text) { &[("$", "zx")] } else { &[] };
            let powershell = uses_powershell(&text);
            for call in extract_imported_calls(&text, &path, &["zx"], globals) {
                if call.export == "$" {
                    out.extend(call.template.map(|script| PluginInput::SourceScript {
                        file: source.clone(),
                        script: if powershell {
                            without_commands(&script, is_powershell_command)
                        } else {
                            script
                        },
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

/// Whether `text` switches zx to PowerShell: it calls `usePwsh()` or `usePowerShell()`, or sets `$.shell` to a
/// `pwsh` or `powershell` executable.
fn uses_powershell(text: &str) -> bool {
    if text.contains("usePwsh(") || text.contains("usePowerShell(") {
        return true;
    }
    text.match_indices("$.shell").any(|(at, needle)| {
        let rest = text[at + needle.len()..].trim_start();
        let Some(value) = rest.strip_prefix('=').filter(|value| !value.starts_with('=')) else {
            return false;
        };
        let value = value.trim_start();
        let Some(quote) = value.chars().next().filter(|c| matches!(c, '\'' | '"' | '`')) else {
            return false;
        };
        value[1..].split(quote).next().is_some_and(|shell| {
            let program = shell.rsplit(['/', '\\']).next().unwrap_or(shell).to_ascii_lowercase();
            matches!(program.trim_end_matches(".exe"), "pwsh" | "powershell")
        })
    })
}
