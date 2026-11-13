//! MCP: the command and arguments of every server in `.mcp.json`, `.cursor/mcp.json`, `.vscode/mcp.json` or
//! `.codex/config.toml` are a script, run in the server's `cwd`. An argument holding a space is quoted, so it stays one
//! word.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::quote_word;
use crate::support::{script_in, strings, values_of};

/// The `mcp` plugin.
pub struct Mcp;

static SPEC: PluginSpec = PluginSpec {
    id: "mcp",
    enablers: &[
        Enabler::File(".mcp.json"),
        Enabler::File(".cursor/mcp.json"),
        Enabler::File(".vscode/mcp.json"),
        Enabler::File(".codex/config.toml"),
    ],
    root_only: true,
    config: &[
        ".mcp.json",
        ".cursor/mcp.json",
        ".vscode/mcp.json",
        ".codex/config.toml",
    ],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

/// `${workspaceFolder}`, `${CLAUDE_PROJECT_DIR}` and `${CLAUDE_PROJECT_DIR:-…}` stand for the project directory.
fn expand_project_dir(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        if name == "workspaceFolder" || name == "CLAUDE_PROJECT_DIR" || name.starts_with("CLAUDE_PROJECT_DIR:-") {
            out.push('.');
        } else {
            out.push_str(&rest[start..=start + 2 + end]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

impl Plugin for Mcp {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for key in ["mcpServers", "servers", "mcp_servers"] {
            let Some(servers @ ConfigValue::Object(_)) = config.value.get(key) else {
                continue;
            };
            for server in values_of(servers) {
                let Some(command) = server.get("command").and_then(ConfigValue::as_str) else {
                    continue;
                };
                let mut words = vec![quote_word(command)];
                words.extend(server.get("args").map_or_default(strings).into_iter().map(quote_word));
                let script = expand_project_dir(&words.join(" "));
                let dir = server
                    .get("cwd")
                    .and_then(ConfigValue::as_str)
                    .map_or_else(String::new, expand_project_dir);
                out.extend(script_in(&dir, script));
            }
        }
        out
    }
}
