//! npm-package-json-lint: the shared configs of `extends` in its cosmiconfig file or
//! `package.json#npmpackagejsonlint`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, strings};

/// The `npm-package-json-lint` plugin.
pub struct NpmPackageJsonLint;

static SPEC: PluginSpec = PluginSpec {
    id: "npm-package-json-lint",
    enablers: &[Enabler::Dependency("npm-package-json-lint")],
    config: &[
        ".npmpackagejsonlintrc",
        ".config/npmpackagejsonlintrc",
        ".npmpackagejsonlintrc.{json,yaml,yml,js,ts,cjs,mjs}",
        "npmpackagejsonlint.config.{js,ts,cjs,mjs}",
        ".config/npmpackagejsonlintrc.{json,yaml,yml,js,ts,cjs,mjs}",
    ],
    package_json_key: Some("npmpackagejsonlint"),
    binaries: &["npmPkgJsonLint"],
    ..PluginSpec::DEFAULT
};

impl Plugin for NpmPackageJsonLint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("extends")
            .map_or_default(strings)
            .into_iter()
            .filter_map(|id| config_ref(config.base, id))
            .collect()
    }
}
