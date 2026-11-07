//! React Email: the templates under `emails/`, or the directory `email … --dir` names, are entries; the preview
//! commands (`email dev`, `email build`, `email start`) use the preview server package.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{flag_values, positionals, workspace_path};

/// The `react-email` plugin.
pub struct ReactEmail;

static SPEC: PluginSpec = PluginSpec {
    id: "react-email",
    enablers: &[Enabler::Dependency("react-email")],
    entry: &["emails/**/*.tsx"],
    binaries: &["email"],
    ..PluginSpec::DEFAULT
};

const PREVIEW_COMMANDS: &[&str] = &["build", "dev", "start"];

impl Plugin for ReactEmail {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    /// The preview server is `@react-email/ui` from React Email 6 and `@react-email/preview-server` before; either
    /// counts as used, as the installed major version is not known here.
    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for script in cx.scripts.values() {
            let previews = positionals(script, SPEC.binaries, &[]).iter().any(|words| {
                words
                    .first()
                    .is_some_and(|command| PREVIEW_COMMANDS.contains(&command.as_str()))
            });
            if previews {
                for package in ["@react-email/ui", "@react-email/preview-server"] {
                    out.push(PluginInput::OptionalDependency(package.to_owned()));
                }
            }
            for dir in flag_values(script, SPEC.binaries, &["--dir", "-d"]) {
                if let Some(dir) = workspace_path(&dir) {
                    out.push(PluginInput::Entry(format!("{dir}/**/*.tsx")));
                }
            }
        }
        out
    }
}
