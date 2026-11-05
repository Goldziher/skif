//! The target keys of the translation, in one place.
//!
//! Every `skif.toml` key the translators write is named here, and so is every upstream file name they look for. The
//! keys in [`PENDING`] are added to the configuration model by other changes; until a key exists there, the generated
//! text is validated with it removed ([`validation_view`]), and the day the model knows the key the full text is
//! parsed instead and the entry in [`PENDING`] can be deleted.

use toml::{Table, Value};

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

/// The rule that reports configuration that no longer matches anything (knip's configuration hints).
pub(crate) const RULE_STALE_JS_CONFIG: &str = "stale-js-config";

// `[[custom-types]]`
pub(crate) const CUSTOM_TYPES: &str = "custom-types";
pub(crate) const CUSTOM_TYPE_NAME: &str = "name";
pub(crate) const CUSTOM_TYPE_PATH: &str = "path";
pub(crate) const CUSTOM_TYPE_STRATEGY: &str = "strategy";

/// A custom type strategy, spelled as skif spells it.
pub(crate) fn strategy(syncpack: &str) -> Option<&'static str> {
    match syncpack {
        "name@version" => Some("name@version"),
        "version" => Some("version"),
        "versionsByName" => Some("versions-by-name"),
        _ => None,
    }
}

// `[[semver-groups]]`
pub(crate) const SEMVER_IGNORED: &str = "ignored";

/// A dependency type token for the package's own version.
pub(crate) const TYPE_LOCAL: &str = "local";

/// The dependency types that exist in the configuration model today.
const MODEL_TYPES: &[&str] = &["prod", "dev", "peer", "optional"];

/// Keys of the translation that the configuration model does not have yet.
struct Pending {
    /// The table path; `*` stands for any key (a workspace directory).
    table: &'static [&'static str],
    keys: &'static [&'static str],
}

const PENDING: &[Pending] = &[
    Pending {
        table: &["js"],
        keys: &[
            JS_IGNORE_WORKSPACES,
            JS_IGNORE_UNRESOLVED,
            JS_IGNORE_EXPORTS_USED_IN_FILE,
            JS_IGNORE_MEMBERS,
        ],
    },
    Pending {
        table: &["js", "workspaces", "*"],
        keys: &[WS_PATHS, WS_IGNORE_UNRESOLVED, WS_INCLUDE_ENTRY_EXPORTS, WS_PLUGINS],
    },
    Pending {
        table: &["rules"],
        keys: &[RULE_STALE_JS_CONFIG],
    },
];

/// `table` as the current configuration model reads it: the pending keys, the custom types, the `local` and custom
/// dependency type tokens and the `ignored` semver groups are replaced by what the model accepts today.
pub(crate) fn validation_view(table: &Table) -> Table {
    let mut view = table.clone();
    for pending in PENDING {
        strip(&mut view, pending.table, pending.keys);
    }
    view.remove(CUSTOM_TYPES);
    if let Some(Value::Table(policy)) = view.get_mut("policy") {
        known_types(policy);
    }
    for group in ["version-groups", "semver-groups"] {
        if let Some(Value::Array(groups)) = view.get_mut(group) {
            for entry in groups.iter_mut().filter_map(Value::as_table_mut) {
                known_types(entry);
            }
        }
    }
    if let Some(Value::Array(groups)) = view.get_mut("semver-groups") {
        for entry in groups.iter_mut().filter_map(Value::as_table_mut) {
            if entry.remove(SEMVER_IGNORED).is_some() && !entry.contains_key("range") {
                entry.insert("range".to_owned(), Value::String(String::new()));
            }
        }
    }
    view
}

fn strip(table: &mut Table, path: &[&str], keys: &[&str]) {
    match path.split_first() {
        None => {
            for key in keys {
                table.remove(*key);
            }
        }
        Some((&"*", rest)) => {
            for value in table.values_mut() {
                if let Value::Table(inner) = value {
                    strip(inner, rest, keys);
                }
            }
        }
        Some((head, rest)) => {
            if let Some(Value::Table(inner)) = table.get_mut(*head) {
                strip(inner, rest, keys);
            }
        }
    }
}

/// Removes the dependency type tokens the model does not know from `dependency-types`.
fn known_types(table: &mut Table) {
    if let Some(Value::Array(tokens)) = table.get_mut("dependency-types") {
        tokens.retain(|token| {
            token
                .as_str()
                .is_some_and(|token| MODEL_TYPES.contains(&token.trim_start_matches('!')))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Table {
        text.parse().unwrap()
    }

    #[test]
    fn pending_keys_and_tokens_are_removed() {
        let source = r#"
[js]
entry = ["a"]
ignore-members = ["x"]

[js.workspaces."apps/web"]
entry = ["b"]
paths = { "@/*" = ["./src/*"] }

[rules]
stale-js-config = "error"
unused-files = "error"

[[custom-types]]
name = "engines"

[[version-groups]]
dependency-types = ["prod", "local", "!engines"]

[[semver-groups]]
ignored = true
dependency-types = ["dev"]
"#;
        let view = validation_view(&table(source));
        let expected = table(
            r#"
[js]
entry = ["a"]

[js.workspaces."apps/web"]
entry = ["b"]

[rules]
unused-files = "error"

[[version-groups]]
dependency-types = ["prod"]

[[semver-groups]]
range = ""
dependency-types = ["dev"]
"#,
        );
        assert_eq!(view, expected);
    }

    #[test]
    fn strategies_use_the_skif_spelling() {
        assert_eq!(strategy("versionsByName"), Some("versions-by-name"));
        assert_eq!(strategy("name~version"), None);
    }
}
