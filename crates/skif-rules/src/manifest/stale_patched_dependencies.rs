use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use skif_config::Selector;
use skif_core::{Category, DiagnosticBuilder, Fixability, Location, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, OptionsError, Rule, parse_options};
use skif_specifier::Specifier;
use skif_workspace::{PnpmLockfile, split_key};

use super::support::key;
use crate::lockfile;

/// Metadata of `stale-patched-dependencies`.
pub static META: RuleMeta = RuleMeta::new(
    "stale-patched-dependencies",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::None,
    "A `patchedDependencies` entry no longer applies, names a missing file or lies outside the allowed paths, or a patch file is left over.",
);

/// The directory pnpm's `pnpm patch-commit` writes to.
const DEFAULT_PATCH_DIR: &str = "patches";

/// Options of `stale-patched-dependencies`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct StalePatchedDependenciesOptions {
    /// Directories, relative to the workspace root, searched for `*.patch` files no entry references. Empty means the
    /// directories of the referenced patches and `patches`.
    pub patch_dirs: Vec<String>,
    /// Globs a patch file must match, relative to the workspace root (`patches/**`); empty allows any path.
    pub allowed_patch_paths: Vec<String>,
}

/// Reports `patchedDependencies` entries of the root `pnpm-workspace.yaml` and the root `package.json`
/// (`pnpm.patchedDependencies`) that have gone stale.
///
/// An entry is stale when `pnpm-lock.yaml` resolves no snapshot of its `name@version` (the patch stopped applying after
/// a bump) or resolves it without a patch hash, when its patch file does not exist, or when the file lies outside
/// `allowed-patch-paths`. A `*.patch` file in the patch directories that no entry references is reported too. The rule
/// reads nothing in a workspace that declares no patches and configures no `patch-dirs`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StalePatchedDependencies;

/// One declared patch: its key, its file and where the key is written.
struct Entry {
    key: String,
    path: String,
    location: Location,
}

impl Rule for StalePatchedDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn options_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "patch-dirs": { "type": "array", "items": { "type": "string" }, "default": [] },
                "allowed-patch-paths": { "type": "array", "items": { "type": "string" }, "default": [] },
            },
        }))
    }

    fn validate_options(&self, options: &toml::Table) -> Result<(), OptionsError> {
        let options = parse_options::<StalePatchedDependenciesOptions>(options)?;
        Selector::new(options.allowed_patch_paths)
            .map(|_| ())
            .map_err(|error| OptionsError::for_key("allowed-patch-paths", error.to_string()))
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let options = cx
            .config
            .rule(META.id)
            .and_then(|rule| parse_options::<StalePatchedDependenciesOptions>(&rule.options).ok())
            .unwrap_or_default();
        let entries = declared(cx);
        if entries.is_empty() && options.patch_dirs.is_empty() {
            return;
        }
        let root = &cx.workspace.root;
        let allowed = Selector::new(options.allowed_patch_paths.clone()).unwrap_or_default();
        for entry in &entries {
            let file = normalize(&entry.path);
            if !cx.fs().is_file(&root.join(&file)) {
                out.emit(
                    subject(
                        diagnostic(
                            &META,
                            format!("the patch file `{}` of `{}` does not exist", entry.path, entry.key),
                            entry.location.clone(),
                        ),
                        &entry.key,
                    )
                    .help("restore the file, or remove the entry if the patch is no longer needed")
                    .build(),
                );
            }
            if !allowed.is_empty() && !allowed.matches(&file, false) {
                out.emit(
                    subject(
                        diagnostic(
                            &META,
                            format!(
                                "the patch file `{}` of `{}` lies outside `allowed-patch-paths` ({})",
                                entry.path,
                                entry.key,
                                options.allowed_patch_paths.join(", ")
                            ),
                            entry.location.clone(),
                        ),
                        &entry.key,
                    )
                    .help("move the patch under an allowed path, which build contexts copy, and update the entry")
                    .build(),
                );
            }
        }
        if let Some(first) = entries.first() {
            let loaded = lockfile::load(cx);
            match loaded.get(cx, &META, "the `patchedDependencies` entries", first.location.clone()) {
                Ok(lock) => {
                    for entry in &entries {
                        if let Some(message) = unapplied(lock, &entry.key) {
                            out.emit(
                                subject(diagnostic(&META, message, entry.location.clone()), &entry.key)
                                    .help("update the key to the resolved version and run `pnpm install`, or remove the entry")
                                    .build(),
                            );
                        }
                    }
                }
                Err(diagnostic) => out.emit(diagnostic),
            }
        }
        let referenced: BTreeSet<String> = entries.iter().map(|entry| normalize(&entry.path)).collect();
        let dirs: BTreeSet<String> = if options.patch_dirs.is_empty() {
            referenced
                .iter()
                .map(|file| file.rsplit_once('/').map_or_else(String::new, |(dir, _)| dir.to_owned()))
                .chain([DEFAULT_PATCH_DIR.to_owned()])
                .collect()
        } else {
            options.patch_dirs.iter().map(|dir| normalize(dir)).collect()
        };
        for dir in dirs {
            let path = if dir.is_empty() { root.clone() } else { root.join(&dir) };
            let Ok(listing) = cx.fs().read_dir(&path) else {
                continue;
            };
            let mut orphans: Vec<String> = listing
                .iter()
                .filter(|item| item.is_file)
                .filter_map(|item| item.path.file_name()?.to_str().map(str::to_owned))
                .filter(|name| name.ends_with(".patch"))
                .map(|name| if dir.is_empty() { name } else { format!("{dir}/{name}") })
                .filter(|file| !referenced.contains(file))
                .collect();
            orphans.sort();
            for file in orphans {
                out.emit(
                    diagnostic(
                        &META,
                        format!("`{file}` is not referenced by any `patchedDependencies` entry"),
                        Location::new(PathBuf::from(&file), None),
                    )
                    .help("delete the leftover patch file, or reference it from `patchedDependencies`")
                    .build(),
                );
            }
        }
    }
}

/// The entries of the root `pnpm-workspace.yaml`, then those of the root `package.json`, in file order.
fn declared(cx: &LintContext<'_>) -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Some(yaml) = &cx.workspace.pnpm_workspace
        && let Ok(document) = serde_saphyr::from_str::<Value>(without_bom(&yaml.source))
        && let Some(Value::Object(patches)) = document.get("patchedDependencies")
    {
        for (name, path) in patches {
            if let Some(path) = path.as_str() {
                entries.push(Entry {
                    key: name.clone(),
                    path: path.to_owned(),
                    location: Location::new(yaml.path.clone(), yaml.entry_span("patchedDependencies", name)),
                });
            }
        }
    }
    let root = cx.workspace.root_package();
    if cx.workspace.has_root_manifest()
        && let Some(Value::Object(patches)) = root.manifest.value().pointer("/pnpm/patchedDependencies")
    {
        for (name, path) in patches {
            if let Some(path) = path.as_str() {
                let pointer = [key("pnpm"), key("patchedDependencies"), key(name)];
                entries.push(Entry {
                    key: name.clone(),
                    path: path.to_owned(),
                    location: Location::new(root.manifest.path.clone(), root.manifest.field_key_span(&pointer)),
                });
            }
        }
    }
    entries
}

/// Why the lockfile shows that the patch keyed `key` does not apply, or `None` when it applies.
fn unapplied(lock: &PnpmLockfile, key: &str) -> Option<String> {
    // `name@1.2.3` patches that release; `name` or `name@^1` (pnpm 10) patch every release they cover.
    let (name, version) = match split_key(key) {
        Some((name, version, _)) => (name, Some(version)),
        None => (key, None),
    };
    let exact = version.filter(|version| matches!(*Specifier::new(version), Specifier::Exact(_)));
    let mut resolved: BTreeSet<&str> = BTreeSet::new();
    let mut patched = false;
    for snapshot in lock.snapshots.values().filter(|snapshot| snapshot.name == name) {
        resolved.insert(&snapshot.version);
        if exact.is_none_or(|exact| snapshot.version == exact) && snapshot.patch_hash.is_some() {
            patched = true;
        }
    }
    let covered = exact.is_none_or(|exact| resolved.contains(exact));
    if !covered || resolved.is_empty() {
        let others = if resolved.is_empty() {
            format!("no release of `{name}`")
        } else {
            let versions: Vec<_> = resolved.iter().map(|version| format!("`{version}`")).collect();
            format!("`{name}` at {}", versions.join(", "))
        };
        return Some(format!(
            "`{key}` is patched but `{}` resolves no `{key}`; it resolves {others}",
            lock.path.display()
        ));
    }
    let hashed = lock
        .patched_dependencies
        .get(key)
        .is_some_and(|patch| patch.hash.is_some());
    if !patched || !hashed {
        return Some(format!(
            "`{key}` is patched but `{}` records no patch hash for it, so the patch is not applied",
            lock.path.display()
        ));
    }
    None
}

fn without_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Names the patched package of `key` and, for `name@version`, the version, as diagnostics about a dependency do.
fn subject(builder: DiagnosticBuilder, key: &str) -> DiagnosticBuilder {
    match split_key(key) {
        Some((name, version, _)) => builder.dependency(name).specifier(version),
        None => builder,
    }
}

/// A workspace-relative path with `/` separators and without `.` or `..` segments that cancel out.
fn normalize(path: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(&path.replace('\\', "/")).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    parts.push("..".to_owned());
                }
            }
            _ => {}
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use skif_core::Diagnostic;

    use super::*;
    use crate::testing::{config, lint, located, root};

    /// `left@1.0.0` is patched; `gone` was bumped to `2.1.0`; `stale@1.0.0` resolves without its patch.
    const LOCK: &str = "lockfileVersion: '9.0'

patchedDependencies:
  left@1.0.0:
    hash: abc123
    path: patches/left@1.0.0.patch

importers:

  .:
    dependencies:
      gone:
        specifier: ^2.0.0
        version: 2.1.0
      left:
        specifier: 1.0.0
        version: 1.0.0(patch_hash=abc123)
      stale:
        specifier: 1.0.0
        version: 1.0.0

packages:

  gone@2.1.0:
    resolution: {integrity: sha512-gone}

  left@1.0.0:
    resolution: {integrity: sha512-left}

  stale@1.0.0:
    resolution: {integrity: sha512-stale}

snapshots:

  gone@2.1.0: {}

  left@1.0.0(patch_hash=abc123): {}

  stale@1.0.0: {}
";

    const PATCH: &str = "--- a/index.js\n+++ b/index.js\n";

    /// The patch file of `left@1.0.0`.
    const LEFT: (&str, &str) = ("patches/left@1.0.0.patch", PATCH);

    /// Lints a workspace whose `pnpm-workspace.yaml` declares `patches` (the lines under `patchedDependencies:`), with
    /// `extra` files besides the root manifest and a member.
    fn stale(patches: &str, extra: &[(&'static str, &'static str)], toml: &str) -> Vec<(Diagnostic, String)> {
        let root_json = root("");
        let yaml = format!("packages:\n  - packages/*\npatchedDependencies:\n{patches}");
        let mut files = vec![
            ("package.json", root_json.as_str()),
            ("pnpm-workspace.yaml", yaml.as_str()),
            ("packages/app/package.json", r#"{"name":"app","version":"1.0.0"}"#),
        ];
        files.extend_from_slice(extra);
        lint(META.id, &files, &config(toml))
            .into_iter()
            .map(|diagnostic| {
                let text = diagnostic
                    .location
                    .span
                    .map(|_| located(&files, &diagnostic).to_owned())
                    .unwrap_or_default();
                (diagnostic, text)
            })
            .collect()
    }

    fn messages(found: &[(Diagnostic, String)]) -> Vec<&str> {
        found.iter().map(|(diagnostic, _)| diagnostic.message.as_str()).collect()
    }

    #[test]
    fn an_applied_patch_reports_nothing() {
        let found = stale("  left@1.0.0: patches/left@1.0.0.patch\n", &[("pnpm-lock.yaml", LOCK), LEFT], "");
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_key_whose_release_the_lockfile_no_longer_resolves_is_reported_at_the_key() {
        let found = stale(
            "  gone@2.0.0: patches/gone@2.0.0.patch\n",
            &[("pnpm-lock.yaml", LOCK), ("patches/gone@2.0.0.patch", PATCH)],
            "",
        );
        assert_eq!(
            messages(&found),
            ["`gone@2.0.0` is patched but `pnpm-lock.yaml` resolves no `gone@2.0.0`; it resolves `gone` at `2.1.0`"]
        );
        assert_eq!(found[0].0.location.path, Path::new("pnpm-workspace.yaml"));
        assert_eq!(found[0].1, "gone@2.0.0: patches/gone@2.0.0.patch");
        assert_eq!(found[0].0.dependency.as_deref(), Some("gone"));
    }

    #[test]
    fn a_resolved_release_without_a_patch_hash_is_reported() {
        let found = stale(
            "  stale@1.0.0: patches/stale@1.0.0.patch\n",
            &[("pnpm-lock.yaml", LOCK), ("patches/stale@1.0.0.patch", PATCH)],
            "",
        );
        assert_eq!(
            messages(&found),
            ["`stale@1.0.0` is patched but `pnpm-lock.yaml` records no patch hash for it, so the patch is not applied"]
        );
    }

    #[test]
    fn a_missing_patch_file_is_reported() {
        let found = stale(
            "  left@1.0.0: patches/left@1.0.0.patch\n  stale@1.0.0: patches/missing.patch\n",
            &[("pnpm-lock.yaml", LOCK), LEFT],
            "",
        );
        assert_eq!(
            messages(&found)[0],
            "the patch file `patches/missing.patch` of `stale@1.0.0` does not exist"
        );
        assert_eq!(found[0].1, "stale@1.0.0: patches/missing.patch");
    }

    #[test]
    fn a_patch_file_no_entry_references_is_reported() {
        let found = stale(
            "  left@1.0.0: ./patches/left@1.0.0.patch\n",
            &[
                ("pnpm-lock.yaml", LOCK),
                LEFT,
                ("patches/old@0.9.0.patch", PATCH),
                ("patches/README.md", "notes"),
            ],
            "",
        );
        assert_eq!(
            messages(&found),
            ["`patches/old@0.9.0.patch` is not referenced by any `patchedDependencies` entry"]
        );
        assert_eq!(found[0].0.location.path, Path::new("patches/old@0.9.0.patch"));
        // A configured directory replaces the default ones.
        let toml = "[rules.stale-patched-dependencies]\npatch-dirs = [\"vendor\"]\n";
        let found = stale(
            "  left@1.0.0: patches/left@1.0.0.patch\n",
            &[
                ("pnpm-lock.yaml", LOCK),
                LEFT,
                ("patches/old@0.9.0.patch", PATCH),
                ("vendor/orphan.patch", PATCH),
            ],
            toml,
        );
        assert_eq!(
            messages(&found),
            ["`vendor/orphan.patch` is not referenced by any `patchedDependencies` entry"]
        );
    }

    #[test]
    fn a_patch_outside_the_allowed_paths_is_reported() {
        let toml = "[rules.stale-patched-dependencies]\nallowed-patch-paths = [\"patches/**\"]\n";
        let found = stale(
            "  left@1.0.0: vendor/left.patch\n",
            &[("pnpm-lock.yaml", LOCK), ("vendor/left.patch", PATCH)],
            toml,
        );
        assert_eq!(
            messages(&found),
            ["the patch file `vendor/left.patch` of `left@1.0.0` lies outside `allowed-patch-paths` (patches/**)"]
        );
        let inside = stale("  left@1.0.0: patches/left@1.0.0.patch\n", &[("pnpm-lock.yaml", LOCK), LEFT], toml);
        assert!(inside.is_empty(), "{inside:#?}");
    }

    #[test]
    fn entries_of_the_root_package_json_point_at_their_key() {
        let root_json = root(r#","pnpm":{"patchedDependencies":{"gone@2.0.0":"patches/gone@2.0.0.patch"}}"#);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/app/package.json", r#"{"name":"app","version":"1.0.0"}"#),
            ("patches/gone@2.0.0.patch", PATCH),
            ("pnpm-lock.yaml", LOCK),
        ];
        let found = lint(META.id, &files, &config(""));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].location.path, Path::new("package.json"));
        assert_eq!(located(&files, &found[0]), r#""gone@2.0.0""#);
    }

    #[test]
    fn without_a_lockfile_or_with_a_broken_one_the_resolution_check_is_reported() {
        let found = stale("  left@1.0.0: patches/left@1.0.0.patch\n", &[LEFT], "");
        assert_eq!(
            messages(&found),
            ["there is no `pnpm-lock.yaml`, so nothing is checked for the `patchedDependencies` entries"]
        );
        assert_eq!(found[0].0.location.path, Path::new("pnpm-workspace.yaml"));
        let found = stale(
            "  left@1.0.0: patches/left@1.0.0.patch\n",
            &[("pnpm-lock.yaml", "lockfileVersion: '9.0'\nsnapshots:\n  - a\n"), LEFT],
            "",
        );
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(
            found[0].0.message.starts_with("`pnpm-lock.yaml` cannot be read (`snapshots` must be a mapping)"),
            "{}",
            found[0].0.message
        );
    }

    #[test]
    fn a_workspace_without_patches_reads_nothing() {
        let root_json = root("");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/app/package.json", r#"{"name":"app","version":"1.0.0"}"#),
            ("patches/old.patch", PATCH),
            ("pnpm-lock.yaml", "not: [valid\n"),
        ];
        assert!(lint(META.id, &files, &config("")).is_empty());
    }

    #[test]
    fn invalid_options_are_rejected() {
        let rule = StalePatchedDependencies;
        let table = |text: &str| text.parse::<toml::Table>().unwrap_or_default();
        assert!(rule.validate_options(&table("patch-dirs = [\"patches\"]")).is_ok());
        assert!(rule.validate_options(&table("allowed-patch-paths = [\"{a\"]")).is_err());
        assert!(rule.validate_options(&table("unknown = 1")).is_err());
    }
}
