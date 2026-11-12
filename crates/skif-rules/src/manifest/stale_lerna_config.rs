use skif_core::shell::{self, Word};
use skif_core::{Category, Fixability, Location, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_specifier::Specifier;
use skif_workspace::{LernaFacts, Package};

use super::support::{active_packages, key};
use crate::toolchain::commands::{basename, program, split_spec};

/// Metadata of `stale-lerna-config`.
pub static META: RuleMeta = RuleMeta::new(
    "stale-lerna-config",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::None,
    "`lerna.json` sets `useWorkspaces` or lacks `version`, which Lerna 7 and later reject, or a script runs `lerna bootstrap`, `lerna add` or `lerna link`, which Lerna 9 removed.",
);

/// The Lerna release that removed `useWorkspaces` and fails without `version`.
const LERNA_7: u64 = 7;
/// The Lerna release that removed `bootstrap`, `add` and `link`.
const LERNA_9: u64 = 9;
/// The commands Lerna 9 removed.
const REMOVED_COMMANDS: [&str; 3] = ["bootstrap", "add", "link"];
/// Programs that run the package named after them (with their own flags first).
const RUNNERS: [&str; 7] = ["npx", "pnpx", "bunx", "npm", "pnpm", "yarn", "bun"];
/// Subcommands of the [`RUNNERS`] that run the package named after them.
const RUN_SUBCOMMANDS: [&str; 3] = ["exec", "dlx", "x"];

/// Reports what the installed Lerna rejects: in the root `lerna.json`, `useWorkspaces` (removed in Lerna 7, which fails
/// with `ECONFIGWORKSPACES` while it is set) and a missing `version` (Lerna 7 and later fail with `ENOVERSION`); in
/// the `scripts` of the root and member `package.json` files, `lerna bootstrap`, `lerna add` and `lerna link`, which
/// Lerna 9 removed.
///
/// The Lerna release is read from `node_modules/lerna/package.json` at the root, else from the lowest version the root
/// `package.json` admits for `lerna`; a tag or `*` names no release. Without either nothing is reported, since an
/// older Lerna accepts all of it.
/// A script that runs a pinned Lerna (`npx lerna@6 bootstrap`) is not reported.
#[derive(Debug, Clone, Copy, Default)]
pub struct StaleLernaConfig;

/// The major version of `version`, a version or a range.
fn major(version: &str) -> Option<u64> {
    Specifier::create(version).get_node_version().map(|version| version.major())
}

/// The major Lerna release of the workspace: the installed one, else the lowest the root manifest admits.
fn lerna_major(cx: &LintContext<'_>) -> Option<u64> {
    let installed = cx.workspace.root.join("node_modules").join("lerna").join("package.json");
    let installed = cx
        .fs()
        .read_to_string(&installed)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|manifest| manifest.get("version")?.as_str().and_then(major));
    installed.or_else(|| {
        cx.workspace
            .root_package()
            .dependencies
            .iter()
            .find(|entry| entry.name == "lerna")
            .filter(|entry| !matches!(*entry.parsed, Specifier::Latest(_)))
            .and_then(|entry| entry.parsed.get_node_version())
            .map(|version| version.major())
    })
}

/// The Lerna command `words` run, when that is one Lerna 9 removed: `lerna bootstrap`, also through a runner
/// (`npx lerna add`, `pnpm exec lerna link`). The word that names it.
fn removed_command(words: &[Word]) -> Option<&Word> {
    let mut index = program(words)?;
    if basename(&words.get(index)?.text) != "lerna" {
        if !RUNNERS.contains(&basename(&words.get(index)?.text)) {
            return None;
        }
        index += 1;
        while words
            .get(index)
            .is_some_and(|word| word.text.starts_with('-') || RUN_SUBCOMMANDS.contains(&word.text.as_str()))
        {
            index += 1;
        }
        let word = words.get(index)?;
        if word.dynamic || split_spec(&word.text) != ("lerna", None) {
            return None;
        }
    }
    words
        .get(index + 1..)?
        .iter()
        .find(|word| !word.text.starts_with('-'))
        .filter(|word| !word.dynamic && REMOVED_COMMANDS.contains(&word.text.as_str()))
}

/// The removed Lerna commands each script of `package` runs: the script name and the command.
fn removed_in_scripts(package: &Package) -> Vec<(&str, String)> {
    let Some(scripts) = package.manifest.value().get("scripts").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    scripts
        .iter()
        .filter_map(|(name, script)| Some((name.as_str(), script.as_str()?)))
        .flat_map(|(name, script)| {
            let tokens = shell::tokenize(script);
            tokens
                .segments
                .iter()
                .filter_map(|segment| removed_command(&segment.words))
                .map(|word| (name, word.text.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn report_lerna_json(lerna: &LernaFacts, major: u64, out: &mut Emitter<'_>) {
    if lerna.use_workspaces.is_some() {
        out.emit(
            diagnostic(
                &META,
                format!("`lerna.json` sets `useWorkspaces`, which Lerna {major} rejects (`ECONFIGWORKSPACES`)"),
                Location::new(lerna.path.clone(), lerna.use_workspaces_span),
            )
            .help("delete the key, as `lerna repair` does: Lerna 7 and later use the package manager's workspaces unless `packages` is set")
            .build(),
        );
    }
    if !lerna.has_version {
        out.emit(
            diagnostic(
                &META,
                format!("`lerna.json` has no `version`, which Lerna {major} requires (`ENOVERSION`)"),
                Location::new(lerna.path.clone(), None),
            )
            .help("set `version` to the release version of the packages, or to `\"independent\"`")
            .build(),
        );
    }
}

impl Rule for StaleLernaConfig {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let Some(major) = lerna_major(cx).filter(|major| *major >= LERNA_7) else {
            return;
        };
        if let Some(lerna) = &cx.workspace.facts.lerna {
            report_lerna_json(lerna, major, out);
        }
        if major < LERNA_9 {
            return;
        }
        for package in active_packages(cx.workspace) {
            for (name, command) in removed_in_scripts(package) {
                let span = package.manifest.field_value_span(&[key("scripts"), key(name)]);
                out.emit(
                    diagnostic(
                        &META,
                        format!("script `{name}` runs `lerna {command}`, which Lerna 9 removed"),
                        Location::new(package.manifest.path.clone(), span),
                    )
                    .package(package.display_name())
                    .help("the package manager's workspaces replace it: install and link the members with the package manager's `install`, and add a dependency with its `add` (`npm install <name> -w <member>`)")
                    .build(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, lint, located};

    const ROOT_10: &str = r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"lerna":"^10.1.1"}}"#;
    const MEMBER: &str = r#"{"name":"a"}"#;
    const VERSIONED: &str = r#"{"version": "independent"}"#;

    /// `(path, message)` of every finding, in report order.
    fn findings(files: &[(&str, &str)]) -> Vec<(String, String)> {
        lint(META.id, files, &config(""))
            .iter()
            .map(|d| (d.location.path.to_string_lossy().replace('\\', "/"), d.message.clone()))
            .collect()
    }

    #[test]
    fn use_workspaces_is_reported_at_the_key() {
        let lerna = r#"{"version": "independent", "useWorkspaces": true}"#;
        let files = [
            ("package.json", ROOT_10),
            ("lerna.json", lerna),
            ("packages/a/package.json", MEMBER),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            "`lerna.json` sets `useWorkspaces`, which Lerna 10 rejects (`ECONFIGWORKSPACES`)"
        );
        assert_eq!(located(&files, &diagnostics[0]), r#""useWorkspaces": true"#);
    }

    #[test]
    fn a_false_use_workspaces_is_rejected_too() {
        let files = [
            ("package.json", ROOT_10),
            ("lerna.json", r#"{"version": "1.0.0", "useWorkspaces": false}"#),
        ];
        assert_eq!(
            findings(&files),
            [(
                "lerna.json".to_owned(),
                "`lerna.json` sets `useWorkspaces`, which Lerna 10 rejects (`ECONFIGWORKSPACES`)".to_owned()
            )]
        );
    }

    #[test]
    fn a_missing_version_is_reported_at_the_file() {
        let files = [("package.json", ROOT_10), ("lerna.json", r#"{"packages": ["packages/*"]}"#)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            "`lerna.json` has no `version`, which Lerna 10 requires (`ENOVERSION`)"
        );
        assert!(diagnostics[0].location.span.is_none());
    }

    #[test]
    fn removed_commands_are_reported_in_root_and_member_scripts() {
        let root = r#"{"name":"root","private":true,"workspaces":["packages/*"],"devDependencies":{"lerna":"^10.1.1"},"scripts":{"setup":"lerna bootstrap --hoist","build":"lerna run build --stream","ok":"echo lerna bootstrap"}}"#;
        let member = r#"{"name":"a","scripts":{"deps":"npx lerna add left-pad --scope a && pnpm exec lerna link","pinned":"npx lerna@6 bootstrap","flagged":"yarn lerna --loglevel=silent link"}}"#;
        let files = [
            ("package.json", root),
            ("lerna.json", VERSIONED),
            ("packages/a/package.json", member),
        ];
        assert_eq!(
            findings(&files),
            [
                (
                    "package.json".to_owned(),
                    "script `setup` runs `lerna bootstrap`, which Lerna 9 removed".to_owned()
                ),
                (
                    "packages/a/package.json".to_owned(),
                    "script `deps` runs `lerna add`, which Lerna 9 removed".to_owned()
                ),
                (
                    "packages/a/package.json".to_owned(),
                    "script `deps` runs `lerna link`, which Lerna 9 removed".to_owned()
                ),
                (
                    "packages/a/package.json".to_owned(),
                    "script `flagged` runs `lerna link`, which Lerna 9 removed".to_owned()
                ),
            ]
        );
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(located(&files, &diagnostics[0]), r#""lerna bootstrap --hoist""#);
    }

    #[test]
    fn the_installed_release_decides_over_the_declared_range() {
        let root = r#"{"name":"root","private":true,"devDependencies":{"lerna":"^6.6.0"},"scripts":{"setup":"lerna bootstrap"}}"#;
        let installed = [
            ("package.json", root),
            ("lerna.json", r#"{"useWorkspaces": true}"#),
            ("node_modules/lerna/package.json", r#"{"name":"lerna","version":"9.0.0"}"#),
        ];
        assert_eq!(findings(&installed).len(), 3);
        let declared_only = [("package.json", root), ("lerna.json", r#"{"useWorkspaces": true}"#)];
        assert_eq!(findings(&declared_only), [] as [(String, String); 0]);
    }

    #[test]
    fn lerna_7_and_8_reject_the_settings_but_keep_the_commands() {
        let root = r#"{"name":"root","private":true,"devDependencies":{"lerna":"~8.2.0"},"scripts":{"setup":"lerna bootstrap"}}"#;
        let files = [("package.json", root), ("lerna.json", r#"{"useWorkspaces": true}"#)];
        assert_eq!(
            findings(&files),
            [
                (
                    "lerna.json".to_owned(),
                    "`lerna.json` sets `useWorkspaces`, which Lerna 8 rejects (`ECONFIGWORKSPACES`)".to_owned()
                ),
                (
                    "lerna.json".to_owned(),
                    "`lerna.json` has no `version`, which Lerna 8 requires (`ENOVERSION`)".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn nothing_is_reported_without_a_known_lerna_release() {
        let unknown = [
            (
                "package.json",
                r#"{"name":"root","private":true,"scripts":{"setup":"lerna bootstrap"}}"#,
            ),
            ("lerna.json", r#"{"useWorkspaces": true}"#),
        ];
        assert_eq!(findings(&unknown), [] as [(String, String); 0]);
        let tagged = [
            (
                "package.json",
                r#"{"name":"root","private":true,"devDependencies":{"lerna":"latest"},"scripts":{"setup":"lerna bootstrap"}}"#,
            ),
            ("lerna.json", r#"{"useWorkspaces": true}"#),
        ];
        assert_eq!(findings(&tagged), [] as [(String, String); 0]);
    }

    #[test]
    fn a_current_lerna_json_is_clean() {
        let files = [
            ("package.json", ROOT_10),
            ("lerna.json", r#"{"version": "1.2.0", "packages": ["packages/*"]}"#),
            ("packages/a/package.json", MEMBER),
        ];
        assert_eq!(findings(&files), [] as [(String, String); 0]);
    }
}
