use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_core::{Category, Fix, FixOp, FixSafety, Fixability, Location, RuleMeta, Severity, diagnostic, slash};
use skif_lint::{Emitter, LintContext, OptionsError, Rule};
use skif_workspace::{BUN_LOCKFILE, LOCKFILES, Package, PackageManager, Workspace};

use super::allow_list::{self, AllowList};
use super::lockfile_consumers;
use crate::paths::parent;

/// Metadata of `stray-lockfile`.
pub static META: RuleMeta = RuleMeta::new(
    "stray-lockfile",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A lockfile other than the root lockfile of the package manager: one in a workspace member, another manager's at the root, or one its manager ignores next to the one it reads (Bun's `bun.lockb` beside `bun.lock`, npm's `package-lock.json` beside `npm-shrinkwrap.json`).",
);

const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

/// Bun's legacy binary lockfile, which Bun does not read while its text `bun.lock` is next to it.
const BUN_BINARY_LOCKFILE: &str = "bun.lockb";

/// npm's lockfile, which npm does not read while `npm-shrinkwrap.json` is next to it.
const NPM_LOCKFILE: &str = "package-lock.json";

/// npm's publishable lockfile, which npm installs from in preference to `package-lock.json`.
const NPM_SHRINKWRAP: &str = "npm-shrinkwrap.json";

/// A root lockfile its package manager ignores while another of its own lockfiles is next to it.
struct Shadow {
    manager: PackageManager,
    /// The lockfile the manager ignores.
    ignored: &'static str,
    /// The lockfile the manager installs from instead.
    read: &'static str,
    /// What the ignored lockfile is, in the message.
    description: &'static str,
}

static SHADOWS: [Shadow; 2] = [
    Shadow {
        manager: PackageManager::Bun,
        ignored: BUN_BINARY_LOCKFILE,
        read: BUN_LOCKFILE,
        description: "Bun's legacy binary lockfile, which Bun ignores",
    },
    Shadow {
        manager: PackageManager::Npm,
        ignored: NPM_LOCKFILE,
        read: NPM_SHRINKWRAP,
        description: "npm's lockfile, which npm ignores",
    },
];

/// The package manager that writes the lockfile called `name`.
fn writer(name: &str) -> &'static str {
    match name {
        NPM_LOCKFILE | NPM_SHRINKWRAP => "npm",
        "yarn.lock" => "Yarn",
        "pnpm-lock.yaml" => "pnpm",
        _ => "Bun",
    }
}

/// Whether `name` is a lockfile `manager` reads at the root of a workspace. A manager skif does not know judges
/// nothing.
fn is_root_lockfile(manager: PackageManager, name: &str) -> bool {
    match manager {
        PackageManager::Npm => matches!(name, NPM_LOCKFILE | NPM_SHRINKWRAP),
        PackageManager::Pnpm => name == "pnpm-lock.yaml",
        PackageManager::Yarn => name == "yarn.lock",
        PackageManager::Bun => matches!(name, BUN_LOCKFILE | BUN_BINARY_LOCKFILE),
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

/// The shadow of the root lockfile `name`: Bun's `bun.lockb` next to a `bun.lock`, or npm's `package-lock.json`
/// next to an `npm-shrinkwrap.json`, each in a workspace of that manager, which installs from the other file.
fn shadow(workspace: &Workspace, name: &str) -> Option<&'static Shadow> {
    SHADOWS.iter().find(|shadow| {
        shadow.ignored == name
            && workspace.package_manager == Some(shadow.manager)
            && workspace
                .facts
                .package_manager_files
                .iter()
                .any(|path| path == Path::new(shadow.read))
    })
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

/// The lockfiles of the workspace that are not the root lockfile of its package manager, without those below the
/// root that a Bazel file or a Dockerfile names (see `lockfile_consumers`).
fn strays<'w>(cx: &LintContext<'w>) -> Vec<Stray<'w>> {
    let candidates = candidates(cx.workspace);
    let below_root: Vec<String> = candidates
        .iter()
        .map(|stray| slash(stray.path))
        .filter(|path| !parent(path).is_empty())
        .collect();
    let consumed = lockfile_consumers::consumed(cx, &below_root);
    candidates
        .into_iter()
        .filter(|stray| !consumed.contains(&slash(stray.path)))
        .collect()
}

/// The lockfiles of the workspace that are not the root lockfile of its package manager.
fn candidates(workspace: &Workspace) -> Vec<Stray<'_>> {
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
                let stray = !is_root_lockfile(manager, name) || shadow(workspace, name).is_some();
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
/// root that another package manager than the workspace's writes, Bun's binary `bun.lockb` next to a `bun.lock`, and
/// npm's `package-lock.json` next to an `npm-shrinkwrap.json`; the manager installs from the other file.
///
/// Every supported manager keeps one lockfile for the whole workspace at its root, so a member's lockfile is a
/// leftover that only misleads the tools run in that directory. Without a known package manager the root lockfiles
/// are not judged. A member that holds its own `pnpm-workspace.yaml` is a root of its own (`nested-workspace-root`
/// reports that), and a member skif ignores or another root of the run lints is left to that root. Files git ignores
/// are not reported.
///
/// Nor is a lockfile below the root that a build installs from, found by reading the build files: in a Bazel
/// repository, a label of it (`"//libs/a:pnpm-lock.yaml"`, also inside a `genrule` command as
/// `$(location //libs/a:pnpm-lock.yaml)`) in the `MODULE.bazel` or `WORKSPACE`, a `.bzl` file they load, or a `BUILD`
/// file of its directory or one above, its relative path in such a `BUILD` file, or a `glob(...)` of its package's
/// `BUILD` file that selects it; when those leave it unnamed, a label of it in any `BUILD` file of the Bazel repository
/// or a `.bzl` file one loads. Docstrings name nothing. Also a `COPY` or `ADD` of it in a Dockerfile of its own
/// directory or one that builds from the workspace root, or a `COPY . .` followed by an install with its package
/// manager in a Dockerfile of its own directory.
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
        let strays = strays(cx);
        for stray in strays.iter().filter(|stray| !allow.allows(stray.path)) {
            let file = slash(stray.path);
            let name = stray
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let in_root = parent(&file).is_empty();
            let shadowed = if in_root { shadow(workspace, name) } else { None };
            let (message, help) = match (stray.package, workspace.package_manager, shadowed) {
                (None, Some(_), Some(shadow)) => (
                    format!("`{file}` is {} next to `{}`", shadow.description, shadow.read),
                    format!(
                        "delete it; {} installs from `{}`, so `{file}` only goes stale",
                        manager_name(shadow.manager),
                        shadow.read
                    ),
                ),
                (None, Some(manager), None) if in_root => (
                    format!(
                        "`{file}` is a lockfile of {}, but the workspace uses {}",
                        writer(name),
                        manager_name(manager)
                    ),
                    "delete it; a second lockfile at the root makes tools disagree about the package manager"
                        .to_owned(),
                ),
                _ => (
                    format!("`{file}` is a lockfile inside the workspace, which installs from the root lockfile"),
                    "delete it; to install the directory on its own, exclude it from the workspace patterns, or keep \
                     it with an entry in `[rules.stray-lockfile] allow`"
                        .to_owned(),
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
            ("npm", vec!["package-lock.json"]),
            ("npm", vec!["npm-shrinkwrap.json"]),
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
    fn an_npm_lockfile_next_to_the_shrinkwrap_is_reported() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"npm@11.6.0","workspaces":["packages/*"]}"#;
        let files = with_root(
            root_json,
            &[
                ("package-lock.json", "{}"),
                ("npm-shrinkwrap.json", "{}"),
                ("packages/a/package.json", MEMBER_A),
            ],
        );
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(paths(&diagnostics), ["package-lock.json"]);
        assert_eq!(
            diagnostics[0].message,
            "`package-lock.json` is npm's lockfile, which npm ignores next to `npm-shrinkwrap.json`"
        );
        assert_eq!(
            diagnostics[0].help.as_deref(),
            Some("delete it; npm installs from `npm-shrinkwrap.json`, so `package-lock.json` only goes stale")
        );
        assert_eq!(diagnostics[0].fixes[0].safety, FixSafety::Unsafe);
        assert!(diagnostics[0].package.is_none());
        // Another manager reads neither, so both are reported as its foreign lockfiles.
        let pnpm_root = root("");
        let files = with_root(
            &pnpm_root,
            &[
                ("pnpm-lock.yaml", ""),
                ("package-lock.json", "{}"),
                ("npm-shrinkwrap.json", "{}"),
            ],
        );
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(paths(&diagnostics), ["npm-shrinkwrap.json", "package-lock.json"]);
        assert_eq!(
            diagnostics[1].message,
            "`package-lock.json` is a lockfile of npm, but the workspace uses pnpm"
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

    /// The repro of a `rules_js` monorepo: one member installed by Bazel, one by its own Dockerfile.
    fn bazel_and_docker_files<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut files = vec![
            (
                "package.json",
                r#"{"name":"root","private":true,"packageManager":"pnpm@10.34.6"}"#,
            ),
            ("pnpm-workspace.yaml", "packages: [libs/a, libs/b, apps/web]\n"),
            ("pnpm-lock.yaml", ""),
            ("libs/a/package.json", MEMBER_A),
            ("libs/a/pnpm-lock.yaml", ""),
            ("libs/b/package.json", MEMBER_B),
            ("libs/b/pnpm-lock.yaml", ""),
            ("apps/web/package.json", r#"{"name":"web"}"#),
            ("apps/web/pnpm-lock.yaml", ""),
            ("MODULE.bazel", MODULE),
            (
                "node_packages.bzl",
                "def deps():\n    npm_translate_lock(name = \"npm_a\", pnpm_lock = \"//libs/a:pnpm-lock.yaml\")\n",
            ),
            (
                "apps/web/Dockerfile",
                "FROM node:22\nCOPY package.json pnpm-lock.yaml ./\nRUN pnpm install\n",
            ),
        ];
        files.extend_from_slice(extra);
        files
    }

    /// A `MODULE.bazel` naming the extension file that declares the lockfile of `libs/a`.
    const MODULE: &str = "module(name = \"repro\")\nnpm = use_extension(\"//:node_packages.bzl\", \"npm\")\n";

    #[test]
    fn a_member_lockfile_a_bazel_label_or_a_dockerfile_names_is_not_reported() {
        let files = bazel_and_docker_files(&[]);
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(paths(&diagnostics), ["libs/b/pnpm-lock.yaml"]);
        let fixed = testing::apply_fixes(&files, &diagnostics);
        let names: Vec<&str> = fixed.iter().map(|(path, _)| path.as_str()).collect();
        assert!(names.contains(&"libs/a/pnpm-lock.yaml") && names.contains(&"apps/web/pnpm-lock.yaml"));
        assert!(!names.contains(&"libs/b/pnpm-lock.yaml"));
    }

    #[test]
    fn a_build_file_of_the_lockfiles_directory_names_it() {
        for build in ["libs/b/BUILD.bazel", "libs/b/BUILD"] {
            for listing in [
                "exports_files([\"pnpm-lock.yaml\"])\n",
                "filegroup(name = \"lock\", srcs = [\":pnpm-lock.yaml\"])\n",
            ] {
                let files = bazel_and_docker_files(&[(build, listing)]);
                assert_eq!(
                    paths(&lint(META.id, &files, &config(""))),
                    [] as [&str; 0],
                    "{build}: {listing}"
                );
            }
        }
        // A plain name in another directory's BUILD file, a commented-out label or an excluded name names nothing.
        let files = bazel_and_docker_files(&[
            ("libs/BUILD.bazel", "exports_files([\"pnpm-lock.yaml\"])\n"),
            ("tools/deps.bzl", "# pnpm_lock = \"//libs/b:pnpm-lock.yaml\"\n"),
            (
                "libs/b/BUILD.bazel",
                "filegroup(name = \"src\", srcs = glob([\"**\"], exclude = [\"pnpm-lock.yaml\"]))\n",
            ),
        ]);
        assert_eq!(paths(&lint(META.id, &files, &config(""))), ["libs/b/pnpm-lock.yaml"]);
    }

    #[test]
    fn bazel_files_outside_a_bazel_repository_name_nothing() {
        let files: Vec<(&str, &str)> = bazel_and_docker_files(&[])
            .into_iter()
            .filter(|(path, _)| *path != "MODULE.bazel")
            .collect();
        assert_eq!(
            paths(&lint(META.id, &files, &config(""))),
            ["libs/a/pnpm-lock.yaml", "libs/b/pnpm-lock.yaml"]
        );
    }

    #[test]
    fn a_lockfile_only_another_packages_build_file_names_is_not_reported() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"pnpm@10.34.6"}"#;
        let repro = |build: &'static str| {
            vec![
                ("package.json", root_json),
                ("pnpm-workspace.yaml", "packages: [libs/a, apps/web]\n"),
                ("pnpm-lock.yaml", ""),
                ("MODULE.bazel", ""),
                ("libs/a/package.json", MEMBER_A),
                ("libs/a/pnpm-lock.yaml", ""),
                ("apps/web/package.json", r#"{"name":"web"}"#),
                ("apps/web/BUILD.bazel", build),
            ]
        };
        let consumers = [
            "npm_translate_lock(pnpm_lock = \"//libs/a:pnpm-lock.yaml\")\n",
            "genrule(name = \"lock\", outs = [\"x\"], cmd = \"cp $(location //libs/a:pnpm-lock.yaml) $@\")\n",
        ];
        for build in consumers {
            assert_eq!(
                paths(&lint(META.id, &repro(build), &config(""))),
                [] as [&str; 0],
                "{build}"
            );
        }
        for build in [
            "\"\"\"//libs/a:pnpm-lock.yaml\"\"\"\n",
            "# npm_translate_lock(pnpm_lock = \"//libs/a:pnpm-lock.yaml\")\n",
        ] {
            assert_eq!(
                paths(&lint(META.id, &repro(build), &config(""))),
                ["libs/a/pnpm-lock.yaml"],
                "{build}"
            );
        }
    }

    #[test]
    fn a_glob_of_the_lockfiles_package_names_it() {
        let files = bazel_and_docker_files(&[(
            "libs/b/BUILD.bazel",
            "filegroup(name = \"locks\", srcs = glob([\"**/*.yaml\"], exclude = [\"tmp/**\"]))\n",
        )]);
        assert_eq!(paths(&lint(META.id, &files, &config(""))), [] as [&str; 0]);
        let excluded = bazel_and_docker_files(&[(
            "libs/b/BUILD.bazel",
            "filegroup(name = \"locks\", srcs = glob([\"**/*.yaml\"], exclude = [\"pnpm-*.yaml\"]))\n",
        )]);
        assert_eq!(paths(&lint(META.id, &excluded, &config(""))), ["libs/b/pnpm-lock.yaml"]);
    }

    #[test]
    fn a_label_with_the_directory_in_its_target_or_the_module_name_names_the_lockfile() {
        for label in [
            "//:libs/b/pnpm-lock.yaml",
            "//libs:b/pnpm-lock.yaml",
            "@repro//libs/b:pnpm-lock.yaml",
        ] {
            let module = format!("{MODULE}npm.npm_translate_lock(pnpm_lock = \"{label}\")\n");
            let files = bazel_and_docker_files(&[]);
            let files: Vec<(&str, &str)> = files
                .into_iter()
                .map(|(path, text)| {
                    if path == "MODULE.bazel" {
                        (path, module.as_str())
                    } else {
                        (path, text)
                    }
                })
                .collect();
            assert_eq!(paths(&lint(META.id, &files, &config(""))), [] as [&str; 0], "{label}");
        }
    }

    /// The stray lockfiles of `files` in a git work tree whose index tracks every file but `untracked`, sorted.
    fn strays_in_git_tree(files: &[(&str, &str)], untracked: &[&str], toml: &str) -> Vec<String> {
        let tracked: Vec<&str> = files
            .iter()
            .map(|(path, _)| *path)
            .filter(|path| !untracked.contains(path))
            .collect();
        let tree = testing::GitTree::new(files, &tracked);
        let mut found = paths(&testing::lint_in(
            META.id,
            &tree,
            &config(toml),
            &RunOptions::default(),
        ));
        found.sort_unstable();
        found
    }

    const GIT_DISCOVERY: &str = "[js]\ndiscovery = \"git\"\n";

    #[test]
    fn under_git_discovery_the_bazel_listing_comes_from_the_git_index() {
        // Only `apps/web/BUILD.bazel` names the lockfile of `libs/a`, and only the listing of the repository finds it.
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"packageManager":"pnpm@10.34.6"}"#,
            ),
            ("pnpm-workspace.yaml", "packages: [libs/a, apps/web]\n"),
            ("pnpm-lock.yaml", ""),
            ("MODULE.bazel", ""),
            ("libs/a/package.json", MEMBER_A),
            ("libs/a/pnpm-lock.yaml", ""),
            ("apps/web/package.json", r#"{"name":"web"}"#),
            (
                "apps/web/BUILD.bazel",
                "npm_translate_lock(pnpm_lock = \"//libs/a:pnpm-lock.yaml\")\n",
            ),
        ];
        let untracked = ["apps/web/BUILD.bazel"];
        assert_eq!(strays_in_git_tree(&files, &[], GIT_DISCOVERY), [] as [&str; 0]);
        assert_eq!(
            strays_in_git_tree(&files, &untracked, GIT_DISCOVERY),
            ["libs/a/pnpm-lock.yaml"],
            "an untracked BUILD file in a directory the index lists is not seen"
        );
        assert_eq!(
            strays_in_git_tree(&files, &untracked, ""),
            [] as [&str; 0],
            "the default discovery walks the directories"
        );
        assert_eq!(
            strays_in_git_tree(&files, &untracked, "[js]\ndiscovery = \"walk\"\n"),
            [] as [&str; 0]
        );
    }

    #[test]
    fn under_git_discovery_the_toolchain_scan_lists_from_the_git_index() {
        // `apps/web/Dockerfile` copies the lockfile of `apps/web`; the scan finds it by listing the member directory.
        let files = bazel_and_docker_files(&[]);
        let untracked = ["apps/web/Dockerfile"];
        assert_eq!(
            strays_in_git_tree(&files, &[], GIT_DISCOVERY),
            ["libs/b/pnpm-lock.yaml"]
        );
        assert_eq!(
            strays_in_git_tree(&files, &untracked, GIT_DISCOVERY),
            ["apps/web/pnpm-lock.yaml", "libs/b/pnpm-lock.yaml"]
        );
        assert_eq!(strays_in_git_tree(&files, &untracked, ""), ["libs/b/pnpm-lock.yaml"]);
    }

    #[test]
    fn under_git_discovery_a_dockerfile_beside_a_lockfile_is_listed_from_the_git_index() {
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"packageManager":"pnpm@10.34.6"}"#,
            ),
            ("pnpm-workspace.yaml", "packages: [tools/*]\n"),
            ("pnpm-lock.yaml", ""),
            ("tools/image/pnpm-lock.yaml", ""),
            ("tools/image/Dockerfile", "FROM node:22\nCOPY pnpm-lock.yaml ./\n"),
        ];
        let untracked = ["tools/image/Dockerfile"];
        assert_eq!(strays_in_git_tree(&files, &[], GIT_DISCOVERY), [] as [&str; 0]);
        assert_eq!(
            strays_in_git_tree(&files, &untracked, GIT_DISCOVERY),
            ["tools/image/pnpm-lock.yaml"]
        );
        assert_eq!(strays_in_git_tree(&files, &untracked, ""), [] as [&str; 0]);
    }

    #[test]
    fn a_dockerfile_building_from_the_workspace_root_names_the_lockfile_it_copies() {
        for (path, dockerfile) in [
            ("Dockerfile", "FROM node:22\nCOPY libs/b/pnpm-lock.yaml ./\n"),
            ("libs/b/Dockerfile", "FROM node:22\nCOPY libs/b/pnpm-lock.yaml ./\n"),
        ] {
            let files = bazel_and_docker_files(&[(path, dockerfile)]);
            assert_eq!(paths(&lint(META.id, &files, &config(""))), [] as [&str; 0], "{path}");
        }
    }

    #[test]
    fn a_dockerfile_beside_a_lockfile_in_a_directory_without_a_manifest_is_read() {
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"packageManager":"pnpm@10.34.6"}"#,
            ),
            ("pnpm-workspace.yaml", "packages: [tools/*]\n"),
            ("pnpm-lock.yaml", ""),
            ("tools/image/pnpm-lock.yaml", ""),
            ("tools/image/Dockerfile", "FROM node:22\nCOPY pnpm-lock.yaml ./\n"),
            ("tools/other/pnpm-lock.yaml", ""),
        ];
        assert_eq!(
            paths(&lint(META.id, &files, &config(""))),
            ["tools/other/pnpm-lock.yaml"]
        );
    }

    #[test]
    fn a_dockerfile_that_copies_its_context_then_installs_names_the_lockfile() {
        let installing = "FROM node:22\nCOPY . .\nRUN corepack enable && pnpm install --frozen-lockfile\n";
        let files = bazel_and_docker_files(&[("libs/b/Dockerfile", installing)]);
        assert_eq!(paths(&lint(META.id, &files, &config(""))), [] as [&str; 0]);
        // Installing first, with another manager, or from another stage does not install from it.
        for dockerfile in [
            "FROM node:22\nRUN pnpm install\nCOPY . .\n",
            "FROM node:22\nCOPY . .\nRUN npm ci\n",
            "FROM node:22\nCOPY --from=build /app/pnpm-lock.yaml ./\nCOPY --from=build . .\nRUN pnpm install\n",
            "FROM node:22\nCOPY . .\nRUN pnpm build\n",
        ] {
            let files = bazel_and_docker_files(&[("libs/b/Dockerfile", dockerfile)]);
            assert_eq!(
                paths(&lint(META.id, &files, &config(""))),
                ["libs/b/pnpm-lock.yaml"],
                "{dockerfile}"
            );
        }
    }

    #[test]
    fn a_dockerfile_elsewhere_built_from_the_members_directory_names_its_lockfile() {
        let dockerfile = (
            "docker/b.Dockerfile",
            "FROM node:22\nCOPY package.json pnpm-lock.yaml ./\nRUN pnpm i\n",
        );
        let compose = "\
services:
  b:
    build:
      context: libs/b
      dockerfile: ../../docker/b.Dockerfile
";
        let workflow = "\
jobs:
  images:
    steps:
      - run: docker buildx build --file docker/b.Dockerfile ./libs/b
";
        for build in [
            ("scripts/images.sh", "docker build -t b -f docker/b.Dockerfile libs/b\n"),
            ("compose.yaml", compose),
            (".github/workflows/images.yml", workflow),
        ] {
            let files = bazel_and_docker_files(&[dockerfile, build]);
            assert_eq!(
                paths(&lint(META.id, &files, &config(""))),
                [] as [&str; 0],
                "{}",
                build.0
            );
        }
        let other_context = bazel_and_docker_files(&[
            dockerfile,
            ("scripts/images.sh", "docker build -f docker/b.Dockerfile libs/a\n"),
        ]);
        assert_eq!(
            paths(&lint(META.id, &other_context, &config(""))),
            ["libs/b/pnpm-lock.yaml"]
        );
    }

    #[test]
    fn an_allow_entry_for_a_lockfile_a_build_names_is_stale() {
        let files = bazel_and_docker_files(&[]);
        let allowing =
            config("[rules.stray-lockfile]\nallow = [\"libs/a/pnpm-lock.yaml\", \"libs/b/pnpm-lock.yaml\"]\n");
        let messages: Vec<String> = lint(META.id, &files, &allowing)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(
            messages,
            ["`libs/a/pnpm-lock.yaml` in `rules.stray-lockfile.allow` names no stray lockfile"]
        );
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
