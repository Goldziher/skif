//! React Email: the templates under `emails/`, or the directory `email … --dir` names, are entries; the preview
//! commands (`email dev`, `email build`, `email start`) use the preview server package.

use skif_js_syntax::ConfigValue;

use crate::config_load::load_config;
use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{flag_values, positionals, workspace_path};
use crate::support::manifest;

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

    /// The preview server is `@react-email/ui` from React Email 6 and `@react-email/preview-server` before. The major
    /// version is the installed one (`node_modules/react-email/package.json` in the workspace or an ancestor), else
    /// the first number of the range the manifest declares, as knip reads it; when neither is known, both count.
    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let preview_servers: &[&str] = match major_version(cx) {
            Some(major) if major >= 6 => &["@react-email/ui"],
            Some(_) => &["@react-email/preview-server"],
            None => &["@react-email/ui", "@react-email/preview-server"],
        };
        let mut out = Vec::new();
        for script in cx.scripts.values() {
            let previews = positionals(script, SPEC.binaries, &[]).iter().any(|words| {
                words
                    .first()
                    .is_some_and(|command| PREVIEW_COMMANDS.contains(&command.as_str()))
            });
            if previews {
                for package in preview_servers {
                    out.push(PluginInput::OptionalDependency((*package).to_owned()));
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

/// The major version of `react-email`: the installed package's, else the declared range's.
fn major_version(cx: &PluginContext<'_>) -> Option<u64> {
    let installed = cx.dir.ancestors().find_map(|dir| {
        let path = dir.join("node_modules/react-email/package.json");
        let source = cx.fs.read_to_string(&path).ok()?;
        let version = load_config(&path, &source)?.get("version")?.as_str()?.to_owned();
        Some(version)
    });
    let declared = || {
        let manifest = manifest(cx)?;
        ["dependencies", "devDependencies"]
            .iter()
            .find_map(|kind| manifest.get(kind)?.get("react-email").and_then(ConfigValue::as_str).map(str::to_owned))
    };
    leading_number(&installed.or_else(declared)?)
}

/// The first run of digits in `version` (`6` in `^6.1.0`).
fn leading_number(version: &str) -> Option<u64> {
    let start = version.find(|c: char| c.is_ascii_digit())?;
    let digits: String = version[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
