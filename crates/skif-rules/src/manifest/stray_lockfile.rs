use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_core::{Category, Fix, FixOp, FixSafety, Fixability, Location, RuleMeta, Severity, diagnostic, slash};
use skif_lint::{Emitter, LintContext, OptionsError, Rule};
use skif_workspace::{LOCKFILES, Package, PackageManager, Workspace};

use super::allow_list::{self, AllowList};

/// Metadata of `stray-lockfile`.
pub static META: RuleMeta = RuleMeta::new(
    "stray-lockfile",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A lockfile other than the root lockfile of the package manager: one in a workspace member, or another manager's at the root.",
);

const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

/// The package manager that writes the lockfile called `name`.
fn writer(name: &str) -> &'static str {
    match name {
        "package-lock.json" | "npm-shrinkwrap.json" => "npm",
        "yarn.lock" => "Yarn",
        "pnpm-lock.yaml" => "pnpm",
        _ => "Bun",
    }
}

/// Whether `name` is a lockfile `manager` reads at the root of a workspace. A manager skif does not know judges
/// nothing.
fn is_root_lockfile(manager: PackageManager, name: &str) -> bool {
    match manager {
        PackageManager::Npm => matches!(name, "package-lock.json" | "npm-shrinkwrap.json"),
        PackageManager::Pnpm => name == "pnpm-lock.yaml",
        PackageManager::Yarn => name == "yarn.lock",
        PackageManager::Bun => matches!(name, "bun.lock" | "bun.lockb"),
        _ => true,
    }
}

/// The name of `manager` in messages.
fn manager_name(manager: PackageManager) -> &'static str {
    match manager {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Yarn => "Yarn",
        PackageManager::Bun => "Bun",
        _ => "the package manager",
    }
}

/// Bun's legacy binary lockfile, which Bun does not read while its text `bun.lock` is next to it.
const BUN_BINARY_LOCKFILE: &str = "bun.lockb";

/// Whether the root lockfile `name` is Bun's binary one next to a `bun.lock` at the root, which Bun installs from.
fn is_shadowed_bun_lockfile(workspace: &Workspace, name: &str) -> bool {
    name == BUN_BINARY_LOCKFILE
        && workspace.package_manager == Some(PackageManager::Bun)
        && workspace
            .facts
            .package_manager_files
            .iter()
            .any(|path| path == Path::new("bun.lock"))
}

/// A lockfile the rule reports unless the allow-list names it.
struct Stray<'w> {
    path: &'w Path,
    /// The member whose directory holds it; `None` at the root and in a selected directory without a manifest.
    package: Option<&'w Package>,
}

/// The directories below the root that hold a `pnpm-workspace.yaml`: roots of their own, whose lockfile is theirs.
fn nested_roots(workspace: &Workspace) -> BTreeSet<&Path> {
    workspace
        .facts
        .package_manager_files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == PNPM_WORKSPACE))
        .filter_map(|path| path.parent())
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect()
}

/// The lockfiles of the workspace that are not the root lockfile of its package manager.
fn strays(workspace: &Workspace) -> Vec<Stray<'_>> {
    let nested = nested_roots(workspace);
    workspace
        .facts
        .package_manager_files
        .iter()
        .map(PathBuf::as_path)
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            if !LOCKFILES.contains(&name) {
                return None;
            }
            let dir = path.parent().unwrap_or_else(|| Path::new(""));
            if dir.as_os_str().is_empty() {
                let manager = workspace.package_manager?;
                let stray = !is_root_lockfile(manager, name) || is_shadowed_bun_lockfile(workspace, name);
                return stray.then_some(Stray { path, package: None });
            }
            if nested.contains(dir) {
                return None;
            }
            let package = workspace.packages().iter().find(|package| package.dir == dir);
            if package.is_some_and(|package| workspace.is_ignored(package.id)) {
                return None;
            }
            Some(Stray { path, package })
        })
        .collect()
}

/// Reports lockfiles a workspace install does not read: every lockfile in a member directory, a lockfile at the
/// root that another package manager than the workspace's writes, and Bun's binary `bun.lockb` next to a `bun.lock`,
/// which Bun installs from instead.
///
/// Every supported manager keeps one lockfile for the whole workspace at its root, so a member's lockfile is a
/// leftover that only misleads the tools run in that directory. Without a known package manager the root lockfiles
/// are not judged. A member that holds its own `pnpm-workspace.yaml` is a root of its own (`nested-workspace-root`
/// reports that), and a member skif ignores or another root of the run lints is left to that root. Files git ignores
/// are not reported.
///
/// Options (`[rules.stray-lockfile] allow`): paths, matched literally first, or globs (`packages/*/package-lock.json`),
/// relative to the workspace root, of lockfiles kept on purpose, each a string or `{ name, reason }`. An entry that
/// matches no stray lockfile is reported as stale.
#[derive(Debug, Clone, Copy, Default)]
pub struct StrayLockfile;

impl Rule for StrayLockfile {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn options_schema(&self) -> Option<serde_json::Value> {
        Some(allow_list::schema())
    }

    fn validate_options(&self, options: &toml::Table) -> Result<(), OptionsError> {
        allow_list::validate(options)
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let workspace = cx.workspace;
        let allow = AllowList::of(cx, &META);
        let strays = strays(workspace);
        for stray in strays.iter().filter(|stray| !allow.allows(stray.path)) {
            let file = slash(stray.path);
            let name = stray
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let at_root = stray.path.parent().is_none_or(|dir| dir.as_os_str().is_empty());
            let (message, help) = match (stray.package, workspace.package_manager) {
                (None, Some(_)) if at_root && is_shadowed_bun_lockfile(workspace, name) => (
                    format!("`{file}` is Bun's legacy binary lockfile, which Bun ignores next to `bun.lock`"),
                    "delete it; Bun installs from `bun.lock`, so the binary lockfile only goes stale",
                ),
                (None, Some(manager)) if at_root => (
                    format!(
                        "`{file}` is a lockfile of {}, but the workspace uses {}",
                        writer(name),
                        manager_name(manager)
                    ),
                    "delete it; a second lockfile at the root makes tools disagree about the package manager",
                ),
                _ => (
                    format!("`{file}` is a lockfile inside the workspace, which installs from the root lockfile"),
                    "delete it; to install the directory on its own, exclude it from the workspace patterns, or keep \
                     it with an entry in `[rules.stray-lockfile] allow`",
                ),
            };
            let mut builder = diagnostic(&META, message, Location::new(stray.path.to_path_buf(), None))
                .help(help)
                .fix(Fix::new(
                    format!("Delete `{file}`"),
                    FixSafety::Unsafe,
                    vec![FixOp::delete_file(stray.path.to_path_buf())],
                ));
            if let Some(package) = stray.package {
                builder = builder.package(package.display_name()).package_id(package.id);
            }
            out.emit(builder.build());
        }
        let found: Vec<&Path> = strays.iter().map(|stray| stray.path).collect();
        allow.report_stale(cx, &META, &found, "stray lockfile", out);
    }
}

#[cfg(test)]
mod tests {
    use skif_lint::RunOptions;

    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, lint_with, root};

    const MEMBER_A: &str = r#"{"name":"a"}"#;
    const MEMBER_B: &str = r#"{"name":"b"}"#;

    fn paths(diagnostics: &[skif_core::Diagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|diagnostic| slash(&diagnostic.location.path))
            .collect()
    }

    fn with_root<'a>(root_json: &'a str, files: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut all = vec![("package.json", root_json)];
        all.extend_from_slice(files);
        all
    }

    #[test]
    fn the_root_lockfile_of_the_package_manager_is_not_reported() {
        let cases = [
            ("pnpm", vec!["pnpm-lock.yaml"]),
            ("npm", vec!["package-lock.json", "npm-shrinkwrap.json"]),
            ("yarn", vec!["yarn.lock"]),
            ("bun", vec!["bun.lock"]),
            ("bun", vec!["bun.lockb"]),
        ];
        for (manager, lockfiles) in cases {
            let root_json = format!(
                r#"{{"name":"root","private":true,"packageManager":"{manager}@1.0.0","workspaces":["packages/*"]}}"#
            );
            let mut files = vec![
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", MEMBER_A),
            ];
            files.extend(lockfiles.iter().map(|name| (*name, "")));
            assert_eq!(paths(&lint(META.id, &files, &config(""))), [] as [&str; 0], "{manager}");
        }
    }

    #[test]
    fn a_bun_binary_lockfile_next_to_the_text_lockfile_is_reported() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"bun@1.4.4","workspaces":["packages/*"]}"#;
        let files = with_root(
            root_json,
            &[
                ("bun.lock", "{}"),
                ("bun.lockb", ""),
                ("packages/a/package.json", MEMBER_A),
            ],
        );
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(paths(&diagnostics), ["bun.lockb"]);
        assert_eq!(
            diagnostics[0].message,
            "`bun.lockb` is Bun's legacy binary lockfile, which Bun ignores next to `bun.lock`"
        );
        assert!(diagnostics[0].package.is_none());
        // Another manager reads neither, so both are reported as its foreign lockfiles.
        let pnpm_root = root("");
        let files = with_root(
            &pnpm_root,
            &[("pnpm-lock.yaml", ""), ("bun.lock", "{}"), ("bun.lockb", "")],
        );
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(paths(&diagnostics), ["bun.lock", "bun.lockb"]);
        assert_eq!(
            diagnostics[1].message,
            "`bun.lockb` is a lockfile of Bun, but the workspace uses pnpm"
        );
    }

    #[test]
    fn every_lockfile_in_a_member_is_reported() {
        let root_json = root("");
        for name in LOCKFILES {
            let lockfile = format!("packages/a/{name}");
            let files = with_root(
                &root_json,
                &[
                    ("pnpm-lock.yaml", ""),
                    ("packages/a/package.json", MEMBER_A),
                    (lockfile.as_str(), ""),
                ],
            );
            let diagnostics = lint(META.id, &files, &config(""));
            assert_eq!(paths(&diagnostics), [lockfile.as_str()], "{name}");
            assert_eq!(diagnostics[0].package.as_deref(), Some("a"), "{name}");
            assert_eq!(
                diagnostics[0].message,
                format!("`{lockfile}` is a lockfile inside the workspace, which installs from the root lockfile"),
                "{name}"
            );
            assert!(diagnostics[0].location.span.is_none());
        }
    }

    #[test]
    fn another_managers_lockfile_at_the_root_is_reported() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("pnpm-lock.yaml", ""),
                ("package-lock.json", "{}"),
                ("yarn.lock", ""),
                ("bun.lockb", ""),
                ("packages/a/package.json", MEMBER_A),
            ],
        );
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(paths(&diagnostics), ["bun.lockb", "package-lock.json", "yarn.lock"]);
        assert_eq!(
            diagnostics[1].message,
            "`package-lock.json` is a lockfile of npm, but the workspace uses pnpm"
        );
        assert!(diagnostics[1].package.is_none());
    }

    #[test]
    fn without_a_known_package_manager_only_member_lockfiles_are_reported() {
        let root_json = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
        let files = with_root(
            root_json,
            &[
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/yarn.lock", ""),
                ("packages/b/package.json", MEMBER_B),
            ],
        );
        assert_eq!(testing::workspace(&files, &config("")).package_manager, None);
        assert_eq!(paths(&lint(META.id, &files, &config(""))), ["packages/a/yarn.lock"]);
    }

    #[test]
    fn a_member_with_its_own_pnpm_workspace_keeps_its_lockfile() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("pnpm-lock.yaml", ""),
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n"),
                ("packages/a/pnpm-lock.yaml", ""),
                ("packages/b/package.json", MEMBER_B),
                ("packages/b/pnpm-lock.yaml", ""),
            ],
        );
        assert_eq!(
            paths(&lint(META.id, &files, &config(""))),
            ["packages/b/pnpm-lock.yaml"]
        );
    }

    #[test]
    fn an_ignored_member_is_not_judged() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/package-lock.json", "{}"),
                ("packages/b/package.json", MEMBER_B),
                ("packages/b/package-lock.json", "{}"),
            ],
        );
        let ignoring = config("[workspace]\nignore-packages = [\"a\"]\n");
        assert_eq!(
            paths(&lint(META.id, &files, &ignoring)),
            ["packages/b/package-lock.json"]
        );
    }

    #[test]
    fn a_gitignored_lockfile_is_not_reported() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                (".git/HEAD", "ref: refs/heads/main\n"),
                (".gitignore", "package-lock.json\n"),
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/package-lock.json", "{}"),
                ("packages/b/package.json", MEMBER_B),
                ("packages/b/yarn.lock", ""),
            ],
        );
        assert_eq!(paths(&lint(META.id, &files, &config(""))), ["packages/b/yarn.lock"]);
    }

    #[test]
    fn allowed_lockfiles_are_not_reported_with_or_without_a_reason() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/package-lock.json", "{}"),
                ("packages/b/package.json", MEMBER_B),
                ("packages/b/yarn.lock", ""),
                ("packages/b/bun.lock", ""),
            ],
        );
        let allowing = config(
            "[rules.stray-lockfile]\nallow = [\n  { name = \"packages/a/package-lock.json\", reason = \"installed on its own by the build\" },\n  \"./packages/b/yarn.lock\",\n]\n",
        );
        assert_eq!(paths(&lint(META.id, &files, &allowing)), ["packages/b/bun.lock"]);
    }

    #[test]
    fn an_allow_entry_that_names_no_stray_lockfile_is_stale() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("pnpm-lock.yaml", ""),
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/package-lock.json", "{}"),
            ],
        );
        let allowing = config(
            "[rules.stray-lockfile]\nallow = [\"packages/a/package-lock.json\", { name = \"packages/gone/yarn.lock\", reason = \"r\" }, \"pnpm-lock.yaml\"]\n",
        );
        let diagnostics = lint(META.id, &files, &allowing);
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "`packages/gone/yarn.lock` in `rules.stray-lockfile.allow` names no stray lockfile",
                "`pnpm-lock.yaml` in `rules.stray-lockfile.allow` names no stray lockfile",
            ]
        );
        assert_eq!(paths(&diagnostics), ["skif.toml", "skif.toml"]);
        assert!(diagnostics.iter().all(|diagnostic| diagnostic.fixes.is_empty()));
    }

    #[test]
    fn stale_entries_of_an_inherited_configuration_are_not_reported() {
        let root_json = root("");
        let files = with_root(&root_json, &[("packages/a/package.json", MEMBER_A)]);
        let allowing = config("[rules.stray-lockfile]\nallow = [\"tools/other-root/package-lock.json\"]\n");
        let inherited = RunOptions::default().with_config_path(Path::new("../skif.toml"));
        assert_eq!(lint_with(META.id, &files, &allowing, &inherited), []);
        let own = RunOptions::default().with_config_path(Path::new("skif.toml"));
        assert_eq!(lint_with(META.id, &files, &allowing, &own).len(), 1);
    }

    #[test]
    fn a_blank_reason_is_rejected() {
        let table: toml::Table = toml::from_str("allow = [{ name = \"a/yarn.lock\", reason = \"  \" }]").unwrap();
        let error = StrayLockfile.validate_options(&table).unwrap_err();
        assert!(error.to_string().contains("allow[0]: `reason` is empty"), "{error}");
        let table: toml::Table = toml::from_str("allow = [{ name = \"a/yarn.lock\", reson = \"r\" }]").unwrap();
        assert!(StrayLockfile.validate_options(&table).is_err());
    }

    #[test]
    fn a_glob_entry_allows_every_lockfile_it_matches() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("pnpm-lock.yaml", ""),
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/package-lock.json", "{}"),
                ("packages/b/package.json", MEMBER_B),
                ("packages/b/package-lock.json", "{}"),
                ("packages/b/yarn.lock", ""),
            ],
        );
        let allowing = config("[rules.stray-lockfile]\nallow = [\"packages/*/package-lock.json\"]\n");
        assert_eq!(paths(&lint(META.id, &files, &allowing)), ["packages/b/yarn.lock"]);
    }

    #[test]
    fn a_glob_entry_that_matches_no_stray_lockfile_is_stale() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("pnpm-lock.yaml", ""),
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/package-lock.json", "{}"),
            ],
        );
        let allowing =
            config("[rules.stray-lockfile]\nallow = [\"packages/*/package-lock.json\", \"tools/**/yarn.lock\"]\n");
        let diagnostics = lint(META.id, &files, &allowing);
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            ["`tools/**/yarn.lock` in `rules.stray-lockfile.allow` names no stray lockfile"]
        );
    }

    #[test]
    fn an_entry_matches_its_path_literally_before_it_is_a_glob() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","workspaces":["app/*"]}"#;
        let files = with_root(
            root_json,
            &[
                ("pnpm-lock.yaml", ""),
                ("app/[id]/package.json", r#"{"name":"id-page"}"#),
                ("app/[id]/package-lock.json", "{}"),
                ("app/b/package.json", MEMBER_B),
                ("app/b/yarn.lock", ""),
            ],
        );
        let allowing = config("[rules.stray-lockfile]\nallow = [\"app/[id]/package-lock.json\"]\n");
        assert_eq!(paths(&lint(META.id, &files, &allowing)), ["app/b/yarn.lock"]);
    }

    #[test]
    fn a_package_selector_is_rejected() {
        let table: toml::Table = toml::from_str("allow = [\"packages/a/yarn.lock\", \"$LOCAL\"]").unwrap();
        let error = StrayLockfile.validate_options(&table).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("allow[1]: `$LOCAL` selects workspace packages, not files; list the files to allow"),
            "{error}"
        );
    }

    #[test]
    fn an_invalid_glob_or_an_exclusion_is_rejected() {
        for (entry, expected) in [
            ("packages/[a/yarn.lock", "allow[0]: "),
            (
                "!packages/a/yarn.lock",
                "allow[0]: `!packages/a/yarn.lock` cannot exclude",
            ),
        ] {
            let table: toml::Table = toml::from_str(&format!("allow = [{entry:?}]")).unwrap();
            let error = StrayLockfile.validate_options(&table).unwrap_err();
            assert!(error.to_string().contains(expected), "{entry}: {error}");
        }
    }

    #[test]
    fn the_fix_deletes_the_lockfile() {
        let root_json = root("");
        let files = with_root(
            &root_json,
            &[
                ("pnpm-lock.yaml", ""),
                ("package-lock.json", "{}"),
                ("packages/a/package.json", MEMBER_A),
                ("packages/a/yarn.lock", ""),
            ],
        );
        assert_fixes_clear(META.id, &files, &config(""));
        let diagnostics = lint(META.id, &files, &config(""));
        assert!(diagnostics.iter().all(|d| d.fixes[0].safety == FixSafety::Unsafe));
        let fixed = testing::apply_fixes(&files, &diagnostics);
        let names: Vec<&str> = fixed.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(names, ["package.json", "packages/a/package.json", "pnpm-lock.yaml"]);
    }
}
