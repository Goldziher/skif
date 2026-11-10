//! husky: git hook scripts are shell scripts whose commands reference binaries and dependencies.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::values_of;

/// The `husky` plugin.
pub struct Husky;

static SPEC: PluginSpec = PluginSpec {
    id: "husky",
    enablers: &[Enabler::Dependency("husky")],
    root_only: true,
    config: &[
        ".husky/{applypatch-msg,pre-applypatch,post-applypatch,pre-commit,pre-merge-commit,prepare-commit-msg,commit-msg,post-commit,pre-rebase,post-checkout,post-merge,pre-push,pre-receive,update,post-receive,post-update,push-to-checkout,pre-auto-gc,post-rewrite,sendemail-validate}",
    ],
    package_json_key: Some("husky"),
    binaries: &["husky"],
    ignore: &[".husky/_/**"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for Husky {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        match config.value {
            ConfigValue::String(hook) => {
                let commands: Vec<&str> = hook
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with(". "))
                    .collect();
                vec![PluginInput::Script(commands.join("\n"))]
            }
            // husky 4: `"husky": { "hooks": { "pre-commit": "lint-staged" } }`
            other => {
                let mut out = vec![PluginInput::Dependency("husky".to_string())];
                if let Some(hooks) = other.get("hooks") {
                    out.extend(
                        values_of(hooks)
                            .into_iter()
                            .filter_map(ConfigValue::as_str)
                            .map(|command| PluginInput::Script(command.to_string())),
                    );
                }
                out
            }
        }
    }
}
