use serde::Deserialize;
use skif_core::{Category, Fix, FixSafety, Fixability, JsonAction, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, OptionsError, Rule, parse_options};

use super::support::{active_members, field_location, json_op, key};

/// Metadata of `member-toolchain-fields`.
pub static META: RuleMeta = RuleMeta {
    id: "member-toolchain-fields",
    category: Category::Manifest,
    default: Some(Severity::Warning),
    fixable: Fixability::Unsafe,
    summary: "Member packages should not declare `packageManager` or `engines`; the root package owns the toolchain.",
};

/// The toolchain fields only the root should declare.
const FIELDS: [&str; 2] = ["packageManager", "engines"];

/// How `member-toolchain-fields` treats toolchain fields in member manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForbidMode {
    /// Members must not declare toolchain fields.
    #[default]
    Forbid,
    /// Members may declare them only when they equal the root's.
    MatchRoot,
}

/// Options of `member-toolchain-fields`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct MemberToolchainFieldsOptions {
    /// Whether members may repeat the root's value.
    pub mode: ForbidMode,
}

/// Reports toolchain fields in the manifests of private members; a published package declares what it supports.
///
/// Options (`[rules.member-toolchain-fields] mode`): `forbid` (default) reports every declaration, `match-root`
/// allows a field whose value equals the root's.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemberToolchainFields;

impl Rule for MemberToolchainFields {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn options_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "mode": { "enum": ["forbid", "match-root"], "default": "forbid" } },
        }))
    }

    fn validate_options(&self, options: &toml::Table) -> Result<(), OptionsError> {
        parse_options::<MemberToolchainFieldsOptions>(options).map(|_| ())
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let mode = cx
            .config
            .rule(META.id)
            .and_then(|rule| parse_options::<MemberToolchainFieldsOptions>(&rule.options).ok())
            .unwrap_or_default()
            .mode;
        let root = &cx.workspace.root_package.manifest.value;
        // A published package states the engines it supports and the manager it is developed with; only the
        // private members of the workspace take their toolchain from the root.
        for package in active_members(cx.workspace).filter(|package| package.private) {
            for field in FIELDS {
                let Some(value) = package.manifest.value.get(field) else {
                    continue;
                };
                if mode == ForbidMode::MatchRoot && root.get(field) == Some(value) {
                    continue;
                }
                let message = match mode {
                    ForbidMode::Forbid => format!("member package declares `{field}`, which the root package owns"),
                    ForbidMode::MatchRoot => format!("`{field}` differs from the root package's"),
                };
                out.emit(
                    diagnostic(&META, message, field_location(package, &[key(field)]))
                        .package(package.display_name())
                        .package_id(package.id)
                        .help(format!(
                            "remove `{field}` from this package.json and declare it once in the root"
                        ))
                        .fix(Fix {
                            title: format!("Remove `{field}`"),
                            safety: FixSafety::Unsafe,
                            ops: vec![json_op(package, vec![key(field)], JsonAction::Remove)],
                        })
                        .build(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, located, root};

    const ROOT_TOOLCHAIN: &str = r#","engines":{"node":">=20"}"#;
    const MATCH_ROOT: &str = "[rules.member-toolchain-fields]\nmode = \"match-root\"\n";

    #[test]
    fn forbid_mode_reports_every_declaration() {
        let root_json = root(ROOT_TOOLCHAIN);
        let cases = [
            ("none", r#"{"name":"a","private":true}"#, vec![]),
            ("engines", r#"{"name":"a","private":true,"engines":{"node":">=20"}}"#, vec!["engines"]),
            (
                "packageManager",
                r#"{"name":"a","private":true,"packageManager":"pnpm@9.0.0"}"#,
                vec!["packageManager"],
            ),
            (
                "both",
                r#"{"name":"a","private":true,"packageManager":"pnpm@9.0.0","engines":{"node":">=20"}}"#,
                vec!["packageManager", "engines"],
            ),
            (
                "empty engines still declared",
                r#"{"name":"a","private":true,"engines":{}}"#,
                vec!["engines"],
            ),
        ];
        for (name, member, expected) in cases {
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", member),
            ];
            let found: Vec<String> = lint(META.id, &files, &config(""))
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect();
            assert_eq!(found.len(), expected.len(), "{name}");
            for (message, field) in found.iter().zip(&expected) {
                assert!(message.contains(&format!("`{field}`")), "{name}: {message}");
            }
        }
    }

    #[test]
    fn match_root_mode_allows_equal_values_only() {
        let root_json = root(ROOT_TOOLCHAIN);
        let cases = [
            (
                "equal",
                r#"{"name":"a","private":true,"engines":{"node":">=20"},"packageManager":"pnpm@9.0.0"}"#,
                0,
            ),
            ("different engines", r#"{"name":"a","private":true,"engines":{"node":">=22"}}"#, 1),
            ("different manager", r#"{"name":"a","private":true,"packageManager":"pnpm@10.0.0"}"#, 1),
            ("equal manager only", r#"{"name":"a","private":true,"packageManager":"pnpm@9.0.0"}"#, 0),
        ];
        for (name, member, expected) in cases {
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", member),
            ];
            assert_eq!(lint(META.id, &files, &config(MATCH_ROOT)).len(), expected, "{name}");
        }
    }

    #[test]
    fn match_root_flags_a_field_the_root_does_not_declare() {
        let root_json = format!(r#"{{"name":"root","private":true,"workspaces":["packages/*"]{ROOT_TOOLCHAIN}}}"#);
        let member = r#"{"name":"a","private":true,"packageManager":"pnpm@9.0.0"}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        assert_eq!(lint(META.id, &files, &config(MATCH_ROOT)).len(), 1);
    }

    #[test]
    fn published_members_may_declare_their_toolchain() {
        let root_json = root(ROOT_TOOLCHAIN);
        let member = r#"{"name":"a","version":"1.0.0","engines":{"node":">=18"},"packageManager":"pnpm@9.0.0"}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        assert_eq!(lint(META.id, &files, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn the_root_itself_is_never_reported() {
        let root_json = root(ROOT_TOOLCHAIN);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", r#"{"name":"a","private":true}"#),
        ];
        assert_eq!(lint(META.id, &files, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn spans_cover_the_field_entry() {
        let root_json = root("");
        let member = r#"{"name":"a","private":true,"engines":{"node":">=20"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(located(&files, &diagnostics[0]), r#""engines":{"node":">=20"}"#);
        assert_eq!(diagnostics[0].package.as_deref(), Some("a"));
    }

    #[test]
    fn ignored_packages_are_skipped() {
        let root_json = root("");
        let member = r#"{"name":"a","private":true,"engines":{}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        assert_eq!(
            lint(META.id, &files, &config("[workspace]\nignore-packages = [\"a\"]\n")),
            [] as [skif_core::Diagnostic; 0]
        );
    }

    #[test]
    fn options_are_validated_by_the_rule() {
        let cases = [
            ("", true, None),
            ("mode = \"forbid\"", true, None),
            ("mode = \"match-root\"", true, None),
            ("moed = \"forbid\"", false, Some("moed")),
            ("mode = \"sometimes\"", false, Some("mode")),
        ];
        for (text, ok, key) in cases {
            let options: toml::Table = text.parse().unwrap();
            let result = MemberToolchainFields.validate_options(&options);
            assert_eq!(result.is_ok(), ok, "{text}");
            if let Err(err) = result {
                assert_eq!(err.key(), key, "{text}: {err}");
            }
        }
    }

    #[test]
    fn registry_rejects_bad_options_naming_rule_and_key() {
        let registry = skif_lint::Registry::new(vec![Box::new(crate::manifest_provider())]).unwrap();
        let bad = config("[rules]\nmember-toolchain-fields = { moed = \"x\" }\n");
        let err = registry.validate_config(&bad).unwrap_err().to_string();
        assert!(err.contains("member-toolchain-fields") && err.contains("moed"), "{err}");
        let good = config("[rules]\nmember-toolchain-fields = { severity = \"info\", mode = \"match-root\" }\n");
        registry.validate_config(&good).unwrap();
        assert!(registry.options_schema("member-toolchain-fields").is_some());
        assert!(registry.options_schema("empty-dependencies").is_none());
    }

    #[test]
    fn fix_removes_the_fields_and_leaves_the_rest() {
        let root_json = root(ROOT_TOOLCHAIN);
        let member = r#"{"name":"a","private":true,"packageManager":"pnpm@9.0.0","version":"1.0.0","engines":{"node":">=20"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::fixed_json(META.id, &files, &config(""), "packages/a/package.json");
        assert_eq!(fixed, serde_json::json!({"name": "a", "version": "1.0.0"}));
        let diagnostics = lint(META.id, &files, &config(""));
        assert!(
            diagnostics
                .iter()
                .all(|d| d.fixes[0].safety == skif_core::FixSafety::Unsafe)
        );
    }
}
