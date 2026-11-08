//! The target keys of the translation, in one place.
//!
//! Every `skif.toml` key the translators write is named here, and so is every upstream file name they look for.

/// `.syncpackrc` and its relatives, in the order syncpack looks for them.
pub(crate) const SYNCPACK_FILES: &[&str] = &[
    ".syncpackrc",
    ".syncpackrc.json",
    ".syncpackrc.yaml",
    ".syncpackrc.yml",
    ".syncpackrc.js",
    ".syncpackrc.cjs",
    ".syncpackrc.mjs",
    ".syncpackrc.ts",
    "syncpack.config.js",
    "syncpack.config.cjs",
    "syncpack.config.mjs",
    "syncpack.config.ts",
];

/// The knip configuration files, in the order knip looks for them.
pub(crate) const KNIP_FILES: &[&str] = &[
    "knip.json",
    "knip.jsonc",
    ".knip.json",
    ".knip.jsonc",
    "knip.ts",
    "knip.js",
    "knip.mjs",
    "knip.cjs",
    "knip.mts",
    "knip.cts",
    "knip.config.ts",
    "knip.config.js",
    "knip.config.mjs",
    "knip.config.cjs",
    "knip.config.mts",
    "knip.config.cts",
];

// `[js]`
pub(crate) const JS_IGNORE_WORKSPACES: &str = "ignore-workspaces";
pub(crate) const JS_IGNORE_UNRESOLVED: &str = "ignore-unresolved";
pub(crate) const JS_IGNORE_EXPORTS_USED_IN_FILE: &str = "ignore-exports-used-in-file";
pub(crate) const JS_IGNORE_MEMBERS: &str = "ignore-members";
pub(crate) const JS_PRODUCTION: &str = "production";
pub(crate) const JS_INCLUDE_ENTRY_EXPORTS: &str = "include-entry-exports";

// `[js.workspaces."<dir>"]`; the root is `"."`.
pub(crate) const WS_PATHS: &str = "paths";
pub(crate) const WS_IGNORE_UNRESOLVED: &str = "ignore-unresolved";
pub(crate) const WS_INCLUDE_ENTRY_EXPORTS: &str = "include-entry-exports";
pub(crate) const WS_PLUGINS: &str = "plugins";

/// The root workspace key.
pub(crate) const ROOT_WORKSPACE: &str = ".";

// `[catalog]`
pub(crate) const CATALOG: &str = "catalog";
pub(crate) const CATALOG_REPORT_UNUSED_ENTRIES: &str = "report-unused-entries";

/// The rule that `[catalog] report-unused-entries` gates (knip's `catalog` issue type).
pub(crate) const RULE_UNUSED_CATALOG_ENTRIES: &str = "unused-catalog-entries";

/// `[policy]` key choosing how the override sections are read.
pub(crate) const POLICY_OVERRIDE_READING: &str = "override-reading";

/// The rule that reports configuration that no longer matches anything (knip's configuration hints).
pub(crate) const RULE_STALE_JS_CONFIG: &str = "stale-js-config";

// `[[custom-types]]`
pub(crate) const CUSTOM_TYPES: &str = "custom-types";
pub(crate) const CUSTOM_TYPE_NAME: &str = "name";
pub(crate) const CUSTOM_TYPE_PATH: &str = "path";
pub(crate) const CUSTOM_TYPE_STRATEGY: &str = "strategy";
pub(crate) const CUSTOM_TYPE_NAME_PATH: &str = "name-path";

/// A custom type strategy, spelled as skif spells it.
pub(crate) fn strategy(syncpack: &str) -> Option<&'static str> {
    match syncpack {
        "name@version" => Some("name@version"),
        "version" => Some("version"),
        "versionsByName" => Some("versions-by-name"),
        "name~version" => Some("name~version"),
        _ => None,
    }
}

// `[[semver-groups]]`
pub(crate) const SEMVER_IGNORED: &str = "ignored";

/// A dependency type token for the package's own version.
pub(crate) const TYPE_LOCAL: &str = "local";

/// The comment after a value that the `exact` profile added, followed by the tools that did not check the rule.
pub(crate) const PROFILE_MARKER: &str = "skif migrate: not checked by ";

/// The comment line that records which tools a file was migrated from.
pub(crate) const SOURCES_PREFIX: &str = "# skif migrate: sources = ";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategies_use_the_skif_spelling() {
        assert_eq!(strategy("versionsByName"), Some("versions-by-name"));
        assert_eq!(strategy("name~version"), Some("name~version"));
        assert_eq!(strategy("nameVersion"), None);
    }
}
