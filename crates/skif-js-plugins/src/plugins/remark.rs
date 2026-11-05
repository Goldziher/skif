//! remark: plugins are packages named `remark-<name>` or by their full name.
//!
//! The name is picked as `load-plugin` does and as knip ports it: `foo` is `remark-foo` or `foo`, `@scope/foo` is
//! `@scope/remark-foo` or `@scope/foo`, `@scope` is `@scope/remark`, and a name with a subpath or one that already
//! starts with `remark-` is taken as written. Of two candidates the declared one is used, `remark-<name>` when neither
//! is.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, name_of, package_name, values_of};

/// The `remark` plugin.
pub struct Remark;

static SPEC: PluginSpec = PluginSpec {
    id: "remark",
    enablers: &[Enabler::Dependency("remark-cli")],
    config: &[
        ".remarkrc",
        ".remarkrc.json",
        ".remarkrc.{js,cjs,mjs}",
        ".remarkrc.{yml,yaml}",
    ],
    package_json_key: Some("remarkConfig"),
    binaries: &["remark"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Remark {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(plugins) = config.value.get("plugins") else {
            return Vec::new();
        };
        values_of(plugins)
            .into_iter()
            .filter_map(name_of)
            .filter_map(|name| {
                if name.starts_with('.') {
                    return module_input(config.base, name, false);
                }
                let candidates = candidates(name);
                let chosen = candidates
                    .iter()
                    .find(|candidate| cx.has_dependency(candidate))
                    .or_else(|| candidates.first())?;
                package_name(chosen).map(PluginInput::Dependency)
            })
            .collect()
    }
}

/// The package names a plugin specifier can stand for, most specific first.
fn candidates(identifier: &str) -> Vec<String> {
    const PREFIX: &str = "remark-";
    let parts: Vec<&str> = identifier.split('/').collect();
    match parts.as_slice() {
        [scope, rest @ ..] if scope.starts_with('@') => match rest {
            [] | [""] => vec![format!("{scope}/remark")],
            [name] if name.starts_with(PREFIX) => vec![identifier.to_owned()],
            [name] => vec![format!("{scope}/{PREFIX}{name}"), identifier.to_owned()],
            _ => vec![identifier.to_owned()],
        },
        [name] if name.starts_with(PREFIX) => vec![identifier.to_owned()],
        [name] => vec![format!("{PREFIX}{name}"), identifier.to_owned()],
        _ => vec![identifier.to_owned()],
    }
}
