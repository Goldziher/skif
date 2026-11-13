//! Buf: the local protoc plugins of `buf.gen.yaml` (or a template passed with `--template`) are commands, so the
//! packages that provide them (`protoc-gen-es`) are used.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{objects, strings};

/// The `buf` plugin.
pub struct Buf;

static SPEC: PluginSpec = PluginSpec {
    id: "buf",
    enablers: &[
        Enabler::Dependency("@bufbuild/buf"),
        Enabler::Dependency("@bufbuild/protoc-gen-es"),
    ],
    config: &["buf.gen.yaml"],
    binaries: &["buf"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for Buf {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// A plugin runs `local` (a command or its words), or `path`, or `protoc-gen-<name>` for a `plugin` or `name`
    /// that is not a remote reference.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for plugin in config.value.get("plugins").map_or_default(objects) {
            let explicit = plugin.get("local").or_else(|| plugin.get("path"));
            let command = match explicit {
                Some(value) => strings(value).join(" "),
                None => match plugin
                    .get("plugin")
                    .or_else(|| plugin.get("name"))
                    .and_then(ConfigValue::as_str)
                {
                    Some(name) if !name.contains('/') => format!("protoc-gen-{name}"),
                    _ => continue,
                },
            };
            if !command.trim().is_empty() {
                out.push(PluginInput::Script(command));
            }
        }
        out
    }
}
