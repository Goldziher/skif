use serde_json::{Value, json};
use skif_core::{
    Category, Fix, FixOp, FixSafety, Fixability, InsertPosition, JsonAction, Location, PathSeg, RuleMeta, Severity,
    diagnostic, slash,
};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::Package;

/// Metadata of `missing-package-manifest`.
pub static META: RuleMeta = RuleMeta::new(
    "missing-package-manifest",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A directory matched by the workspace patterns has no `package.json`, or in a Bun workspace one without a `name`, which Bun does not install.",
);

/// The detail of a Bun member without a `name`.
const NAMELESS_DETAIL: &str = "NamelessBunMember";

/// Reports non-empty directories matched by the workspace patterns that lack a `package.json`.
///
/// Empty directories (left behind by switching branches), hidden directories and `node_modules` are not reported, nor
/// are an Nx project (a directory with a `project.json`) and a grouping directory whose content is only directories
/// the patterns select too (`libs/cart` when `libs/*/*` selects `libs/cart/feature`).
///
/// In a Bun workspace, a member whose `package.json` has no `name` (or an empty one) is reported too, at the manifest
/// with the detail `NamelessBunMember`: Bun skips such a member without a word, so none of its dependencies are
/// installed. Discovery excludes it from analysis ([`DiscoveryFacts::nameless_members`]); the unsafe fix names it
/// after its directory.
///
/// [`DiscoveryFacts::nameless_members`]: skif_workspace::DiscoveryFacts::nameless_members
#[derive(Debug, Clone, Copy, Default)]
pub struct MissingPackageManifest;

/// The finding for `package`, a Bun member without a `name`.
fn nameless(package: &Package, out: &mut Emitter<'_>) {
    let manifest = &package.manifest;
    let name_key = [PathSeg::Key("name".to_owned())];
    let has_key = manifest.value().get("name").is_some();
    let mut builder = diagnostic(
        &META,
        format!(
            "`{}` has no `name`, so Bun does not install it as a workspace package",
            slash(&manifest.path)
        ),
        Location::new(
            manifest.path.clone(),
            has_key.then(|| manifest.field_span(&name_key)).flatten(),
        ),
    )
    .detail(NAMELESS_DETAIL)
    .help("give the package a `name`, or narrow the workspace patterns so the directory is not matched");
    if let Some(name) = package.dir.file_name().and_then(|name| name.to_str()) {
        let op = if has_key {
            FixOp::json(manifest.path.clone(), name_key.to_vec(), JsonAction::Set(Value::from(name)))
        } else {
            FixOp::json(
                manifest.path.clone(),
                Vec::new(),
                JsonAction::insert("name".to_owned(), Value::from(name), InsertPosition::Start),
            )
        };
        builder = builder.fix(Fix::new(
            format!("Name the package `{name}`"),
            FixSafety::Unsafe,
            vec![op],
        ));
    }
    out.emit(builder.build());
}

impl Rule for MissingPackageManifest {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        for dir in &cx.workspace.facts.dirs_without_manifest {
            let manifest = dir.join("package.json");
            let mut builder = diagnostic(
                &META,
                format!("`{}` matches a workspace pattern but has no package.json", slash(dir)),
                Location::new(dir.clone(), None),
            )
            .help("add a package.json, or narrow the workspace patterns so the directory is not matched");
            if let Some(name) = dir.file_name().and_then(|name| name.to_str()) {
                let contents = serde_json::to_string_pretty(&json!({
                    "name": name,
                    "version": "0.0.0",
                    "private": true,
                }));
                if let Ok(contents) = contents {
                    builder = builder.fix(Fix::new(
                        format!("Create `{}`", slash(&manifest)),
                        FixSafety::Unsafe,
                        vec![FixOp::create_file(manifest, format!("{contents}\n"))],
                    ));
                }
            }
            out.emit(builder.build());
        }
        let workspace = cx.workspace;
        for dir in &workspace.facts.nameless_members {
            if let Some(package) = workspace.packages().iter().find(|package| package.dir == *dir) {
                nameless(package, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, root};

    const MEMBER: &str = r#"{"name":"a"}"#;

    #[test]
    fn directory_table() {
        let root_json = root("");
        let cases = [
            (
                "every directory has one",
                vec![("packages/a/package.json", MEMBER)],
                vec![],
            ),
            (
                "non-empty without manifest",
                vec![("packages/a/package.json", MEMBER), ("packages/b/index.js", "")],
                vec!["packages/b"],
            ),
            (
                "several, sorted",
                vec![
                    ("packages/z/x", ""),
                    ("packages/a/package.json", MEMBER),
                    ("packages/m/x", ""),
                ],
                vec!["packages/m", "packages/z"],
            ),
            (
                "hidden and node_modules are not matched",
                vec![
                    ("packages/a/package.json", MEMBER),
                    ("packages/.cache/x", ""),
                    ("packages/node_modules/x", ""),
                ],
                vec![],
            ),
            (
                "unicode directory",
                vec![("packages/a/package.json", MEMBER), ("packages/pä/x", "")],
                vec!["packages/pä"],
            ),
        ];
        for (name, members, expected) in cases {
            let mut files = vec![("package.json", root_json.as_str())];
            files.extend(members);
            let found: Vec<String> = lint(META.id, &files, &config(""))
                .iter()
                .map(|d| d.location.path.to_string_lossy().into_owned())
                .collect();
            assert_eq!(found, expected, "{name}");
        }
    }

    #[test]
    fn grouping_directories_and_nx_projects_need_no_manifest() {
        let nested = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","workspaces":["libs/*","libs/*/*","libs/*/*/*"]}"#;
        let shallow = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","workspaces":["libs/*"]}"#;
        let cases = [
            (
                "a grouping directory of members",
                nested,
                vec![
                    ("libs/cart/feature/package.json", MEMBER),
                    ("libs/cart/ui/package.json", MEMBER),
                ],
                vec![],
            ),
            (
                "grouping directories on two levels",
                nested,
                vec![
                    ("libs/shared/product/data/package.json", MEMBER),
                    ("libs/shared/product/ui/package.json", MEMBER),
                ],
                vec![],
            ),
            (
                "hidden entries and installed dependencies beside the members",
                nested,
                vec![
                    ("libs/cart/feature/package.json", MEMBER),
                    ("libs/cart/.DS_Store", ""),
                    ("libs/cart/node_modules/x/index.js", ""),
                ],
                vec![],
            ),
            (
                "a grouping directory that also holds a file",
                nested,
                vec![("libs/cart/feature/package.json", MEMBER), ("libs/cart/README.md", "")],
                vec!["libs/cart"],
            ),
            (
                "a directory whose subdirectory the patterns do not select",
                shallow,
                vec![("libs/cart/feature/package.json", MEMBER)],
                vec!["libs/cart"],
            ),
            (
                "an Nx project",
                shallow,
                vec![
                    ("libs/web/project.json", r#"{"name":"web"}"#),
                    ("libs/web/src/main.ts", ""),
                ],
                vec![],
            ),
            (
                "a project.json in a subdirectory does not count",
                shallow,
                vec![("libs/web/src/project.json", "{}")],
                vec!["libs/web"],
            ),
        ];
        for (name, root_json, members, expected) in cases {
            let mut files = vec![("package.json", root_json)];
            files.extend(members);
            let found: Vec<String> = lint(META.id, &files, &config(""))
                .iter()
                .map(|d| d.location.path.to_string_lossy().replace('\\', "/"))
                .collect();
            assert_eq!(found, expected, "{name}");
        }
    }

    #[test]
    fn a_bun_member_without_a_name_is_reported_and_named_by_the_fix() {
        let bun_root = r#"{"name":"root","private":true,"packageManager":"bun@1.4.4","workspaces":["packages/*"]}"#;
        let npm_root = r#"{"name":"root","private":true,"packageManager":"npm@11.0.0","workspaces":["packages/*"]}"#;
        let members = [
            ("packages/a/package.json", MEMBER),
            ("packages/blank/package.json", "{\n  \"name\": \"\",\n  \"version\": \"1.0.0\"\n}\n"),
            ("packages/none/package.json", "{\n  \"version\": \"1.0.0\"\n}\n"),
        ];
        let mut files = vec![("package.json", bun_root)];
        files.extend(members);
        let diagnostics = lint(META.id, &files, &config(""));
        let found: Vec<(&str, Option<&str>, &str)> = diagnostics
            .iter()
            .map(|d| {
                (
                    d.location.path.to_str().unwrap(),
                    d.location.span.map(|span| testing::slice(&files, d.location.path.to_str().unwrap(), span)),
                    d.message.as_str(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    "packages/blank/package.json",
                    Some(r#""name": """#),
                    "`packages/blank/package.json` has no `name`, so Bun does not install it as a workspace package"
                ),
                (
                    "packages/none/package.json",
                    None,
                    "`packages/none/package.json` has no `name`, so Bun does not install it as a workspace package"
                ),
            ]
        );
        assert!(diagnostics.iter().all(|d| d.detail.as_deref() == Some(NAMELESS_DETAIL)));
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::apply_fixes(&files, &diagnostics);
        let names: Vec<serde_json::Value> = ["packages/blank/package.json", "packages/none/package.json"]
            .iter()
            .map(|wanted| {
                let (_, text) = fixed.iter().find(|(path, _)| path == wanted).unwrap();
                serde_json::from_str::<serde_json::Value>(text).unwrap()["name"].clone()
            })
            .collect();
        assert_eq!(names, [serde_json::json!("blank"), serde_json::json!("none")]);
        // npm installs a member without a name.
        let mut npm = vec![("package.json", npm_root)];
        npm.extend(members);
        assert_eq!(lint(META.id, &npm, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn the_location_is_the_directory_without_a_span() {
        let root_json = root("");
        let files = [("package.json", root_json.as_str()), ("packages/b/index.js", "")];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].location.path.to_str(), Some("packages/b"));
        assert!(diagnostics[0].location.span.is_none());
    }

    #[test]
    fn fix_creates_a_private_manifest_named_after_the_directory() {
        let root_json = root("");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", MEMBER),
            ("packages/b/index.js", ""),
        ];
        assert_fixes_clear(META.id, &files, &config(""));
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].fixes[0].safety, FixSafety::Unsafe);
        let fixed = testing::apply_fixes(&files, &diagnostics);
        let (_, text) = fixed
            .iter()
            .find(|(path, _)| path == "packages/b/package.json")
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"name": "b", "version": "0.0.0", "private": true})
        );
    }
}
