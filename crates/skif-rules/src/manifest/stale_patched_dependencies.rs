use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use skif_config::Selector;
use skif_core::{Category, DiagnosticBuilder, Fixability, Location, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, OptionsError, Rule, parse_options};
use skif_specifier::Specifier;
use skif_workspace::{BUN_LOCKFILE, BunLockfile, PackageManager, PnpmLockfile, WorkspaceError, split_key};

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
/// (`pnpm.patchedDependencies`) that have gone stale; in a Bun workspace, those of the top-level
/// `patchedDependencies` of the root `package.json`, the only ones Bun reads.
///
/// An entry is stale when `pnpm-lock.yaml` resolves no snapshot of its `name@version` (the patch stopped applying after
/// a bump) or resolves it without a patch hash, when its patch file does not exist, or when the file lies outside
/// `allowed-patch-paths`. In a Bun workspace `bun.lock` decides: it must resolve the release and record the patch in
/// its own `patchedDependencies`, which Bun writes when it applies one. A `*.patch` file in the patch directories that
/// no entry references is reported too. The rule reads nothing in a workspace that declares no patches and configures
/// no `patch-dirs`.
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
        let bun = cx.workspace.package_manager == Some(PackageManager::Bun);
        let entries = if bun { declared_for_bun(cx) } else { declared(cx) };
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
        if let Some(first) = entries.first().filter(|_| bun) {
            check_bun_lock(cx, &entries, first.location.clone(), out);
        } else if let Some(first) = entries.first() {
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
                Err(diagnostic) => out.emit(*diagnostic),
            }
        }
        let referenced: BTreeSet<String> = entries.iter().map(|entry| normalize(&entry.path)).collect();
        let dirs: BTreeSet<String> = if options.patch_dirs.is_empty() {
            referenced
                .iter()
                .map(|file| {
                    file.rsplit_once('/')
                        .map_or_else(String::new, |(dir, _)| dir.to_owned())
                })
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
                .filter(|name| {
                    Path::new(name)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("patch"))
                })
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

/// The entries of the top-level `patchedDependencies` of the root `package.json`, which Bun reads, in file order.
fn declared_for_bun(cx: &LintContext<'_>) -> Vec<Entry> {
    let root = cx.workspace.root_package();
    if !cx.workspace.has_root_manifest() {
        return Vec::new();
    }
    let Some(Value::Object(patches)) = root.manifest.value().get("patchedDependencies") else {
        return Vec::new();
    };
    patches
        .iter()
        .filter_map(|(name, path)| {
            let pointer = [key("patchedDependencies"), key(name)];
            Some(Entry {
                key: name.clone(),
                path: path.as_str()?.to_owned(),
                location: Location::new(root.manifest.path.clone(), root.manifest.field_key_span(&pointer)),
            })
        })
        .collect()
}

/// Checks the entries against `bun.lock`, or reports that nothing is checked when it is missing, binary only or
/// unreadable. `fallback` is where the report points without any lockfile: the first entry.
fn check_bun_lock(cx: &LintContext<'_>, entries: &[Entry], fallback: Location, out: &mut Emitter<'_>) {
    const WHAT: &str = "the `patchedDependencies` entries";
    let root = &cx.workspace.root;
    let (message, location, help) = match BunLockfile::read(cx.fs(), root) {
        Ok(Some(lock)) => {
            for entry in entries {
                if let Some(message) = bun_unapplied(&lock, &entry.key) {
                    out.emit(
                        subject(diagnostic(&META, message, entry.location.clone()), &entry.key)
                            .help("update the key to the resolved version and run `bun install`, or remove the entry")
                            .build(),
                    );
                }
            }
            return;
        }
        Ok(None) if cx.fs().is_file(&root.join("bun.lockb")) => (
            format!("skif reads `{BUN_LOCKFILE}` only; the binary `bun.lockb` is not checked for {WHAT}"),
            Location::new(PathBuf::from("bun.lockb"), None),
            "convert it with `bun install --save-text-lockfile --frozen-lockfile --lockfile-only` and delete `bun.lockb`",
        ),
        Ok(None) => (
            format!("there is no `{BUN_LOCKFILE}`, so nothing is checked for {WHAT}"),
            fallback,
            "run `bun install` to write the lockfile",
        ),
        Err(error) => {
            let reason = match error {
                WorkspaceError::InvalidLockfile { message, .. } => message,
                other => other.to_string(),
            };
            (
                format!("`{BUN_LOCKFILE}` cannot be read ({reason}), so nothing is checked for {WHAT}"),
                Location::new(PathBuf::from(BUN_LOCKFILE), None),
                "fix or regenerate the lockfile with `bun install`",
            )
        }
    };
    out.emit(diagnostic(&META, message, location).help(help).build());
}

/// Why `bun.lock` shows that the patch keyed `key` does not apply, or `None` when it applies.
fn bun_unapplied(lock: &BunLockfile, key: &str) -> Option<String> {
    let (name, version) = match split_key(key) {
        Some((name, version, _)) => (name, Some(version)),
        None => (key, None),
    };
    let resolved = lock.versions_of(name);
    if resolved.is_empty() || version.is_some_and(|version| !resolved.contains(version)) {
        return Some(not_resolved(key, name, &resolved, &lock.path));
    }
    (!lock.patched_dependencies.contains_key(key)).then(|| {
        format!(
            "`{key}` is patched but `{}` records no patch for it, so the patch is not applied",
            lock.path.display()
        )
    })
}

/// The message for a patch keyed `key` whose release the lockfile at `path` does not resolve; `resolved` holds the
/// versions of `name` it does resolve.
fn not_resolved(key: &str, name: &str, resolved: &BTreeSet<&str>, path: &Path) -> String {
    let others = if resolved.is_empty() {
        format!("no release of `{name}`")
    } else {
        let versions: Vec<_> = resolved.iter().map(|version| format!("`{version}`")).collect();
        format!("`{name}` at {}", versions.join(", "))
    };
    format!(
        "`{key}` is patched but `{}` resolves no `{key}`; it resolves {others}",
        path.display()
    )
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
        return Some(not_resolved(key, name, &resolved, &lock.path));
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
            // `..` cancels the segment before it, unless that is a `..` of its own (`../../x`).
            Component::ParentDir if parts.last().is_some_and(|last| last != "..") => {
                parts.pop();
            }
            Component::ParentDir => parts.push("..".to_owned()),
            _ => {}
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use skif_core::Diagnostic;

    #[test]
    fn normalize_cancels_segments_but_keeps_leading_parents() {
        assert_eq!(normalize("./patches/../patches/a.patch"), "patches/a.patch");
        assert_eq!(normalize("../../shared/a.patch"), "../../shared/a.patch");
        assert_eq!(normalize("a/../../b.patch"), "../b.patch");
        assert_eq!(normalize("patches\\b.patch"), "patches/b.patch");
    }

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
        with_located(&files, lint(META.id, &files, &config(toml)))
    }

    /// Each diagnostic with the text of `files` its span covers, empty without a span.
    fn with_located(files: &[(&str, &str)], diagnostics: Vec<Diagnostic>) -> Vec<(Diagnostic, String)> {
        diagnostics
            .into_iter()
            .map(|diagnostic| {
                let text = diagnostic
                    .location
                    .span
                    .map(|_| located(files, &diagnostic).to_owned())
                    .unwrap_or_default();
                (diagnostic, text)
            })
            .collect()
    }

    fn messages(found: &[(Diagnostic, String)]) -> Vec<&str> {
        found
            .iter()
            .map(|(diagnostic, _)| diagnostic.message.as_str())
            .collect()
    }

    #[test]
    fn an_applied_patch_reports_nothing() {
        let found = stale(
            "  left@1.0.0: patches/left@1.0.0.patch\n",
            &[("pnpm-lock.yaml", LOCK), LEFT],
            "",
        );
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
        let inside = stale(
            "  left@1.0.0: patches/left@1.0.0.patch\n",
            &[("pnpm-lock.yaml", LOCK), LEFT],
            toml,
        );
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
            found[0]
                .0
                .message
                .starts_with("`pnpm-lock.yaml` cannot be read (`snapshots` must be a mapping)"),
            "{}",
            found[0].0.message
        );
    }

    /// The `bun.lock` `bun install` (1.4.4) wrote after `bun patch --commit` patched `ms@2.1.2`.
    const BUN_LOCK: &str = r#"{
  "lockfileVersion": 2,
  "configVersion": 1,
  "workspaces": {
    "": {
      "name": "root",
    },
    "packages/app": {
      "name": "app",
      "version": "1.0.0",
      "dependencies": {
        "ms": "^2.1.1",
        "picocolors": "^1.1.0",
      },
    },
  },
  "patchedDependencies": {
    "ms@2.1.2": "patches/ms@2.1.2.patch",
  },
  "overrides": {
    "ms": "2.1.2",
  },
  "packages": {
    "app": ["app@workspace:packages/app"],

    "ms": ["ms@2.1.2", "", {}, "sha512-sGkPx+VjMtmA6MX27oA4FBFELFCZZ4S4XqeGOXCv68tT+jb3vk/RyaKWP0PTKyWtmLSM0b+adUTEvbs1PEaH2w=="],

    "picocolors": ["picocolors@1.1.1", "", {}, "sha512-xceH2snhtb5M9liqDsmEw56le376mTZkEX/jEb/RxNFyegNul7eNslCXP9FDj/Lcu0X8KEyMceP2ntpaHrDEVA=="],
  }
}
"#;

    /// Lints a Bun workspace whose root `package.json` declares the top-level `patchedDependencies` `patches` (the
    /// members of the object), with `extra` files besides the manifests and the patch of `ms@2.1.2`.
    fn bun_stale(patches: &str, extra: &[(&'static str, &'static str)]) -> Vec<(Diagnostic, String)> {
        let root_json = format!(
            r#"{{"name":"root","private":true,"packageManager":"bun@1.4.4","workspaces":["packages/*"],"patchedDependencies":{{{patches}}},"pnpm":{{"patchedDependencies":{{"left@1.0.0":"patches/left@1.0.0.patch"}}}}}}"#
        );
        let mut files = vec![
            ("package.json", root_json.as_str()),
            ("packages/app/package.json", r#"{"name":"app","version":"1.0.0"}"#),
            ("patches/ms@2.1.2.patch", PATCH),
            LEFT,
        ];
        files.extend_from_slice(extra);
        with_located(&files, lint(META.id, &files, &config("")))
    }

    /// What Bun leaves over in every [`bun_stale`] workspace: the patch of the `pnpm.patchedDependencies` it ignores.
    const PNPM_PATCH_LEFT_OVER: &str =
        "`patches/left@1.0.0.patch` is not referenced by any `patchedDependencies` entry";

    #[test]
    fn a_bun_patch_that_bun_lock_records_reports_nothing_and_pnpm_entries_are_not_read() {
        let found = bun_stale(r#""ms@2.1.2":"patches/ms@2.1.2.patch""#, &[("bun.lock", BUN_LOCK)]);
        // `patches/left@1.0.0.patch` belongs to the `pnpm.patchedDependencies` Bun ignores, so it is left over.
        assert_eq!(messages(&found), [PNPM_PATCH_LEFT_OVER]);
        assert_eq!(found[0].0.location.path, Path::new("patches/left@1.0.0.patch"));
    }

    #[test]
    fn a_bun_patch_of_a_release_bun_lock_does_not_resolve_or_does_not_patch_is_reported_at_its_key() {
        let found = bun_stale(
            r#""ms@2.1.2":"patches/ms@2.1.2.patch","picocolors@9.9.9":"patches/picocolors@9.9.9.patch","picocolors@1.1.1":"patches/picocolors@1.1.1.patch""#,
            &[
                ("bun.lock", BUN_LOCK),
                ("patches/picocolors@9.9.9.patch", PATCH),
                ("patches/picocolors@1.1.1.patch", PATCH),
            ],
        );
        assert_eq!(
            messages(&found),
            [
                "`picocolors@9.9.9` is patched but `bun.lock` resolves no `picocolors@9.9.9`; it resolves `picocolors` at `1.1.1`",
                "`picocolors@1.1.1` is patched but `bun.lock` records no patch for it, so the patch is not applied",
                PNPM_PATCH_LEFT_OVER,
            ]
        );
        assert_eq!(found[0].0.location.path, Path::new("package.json"));
        assert_eq!(found[0].1, r#""picocolors@9.9.9""#);
        assert_eq!(found[0].0.dependency.as_deref(), Some("picocolors"));
        assert_eq!(found[1].0.location.path, Path::new("package.json"));
        assert_eq!(found[1].1, r#""picocolors@1.1.1""#);
        assert_eq!(found[1].0.dependency.as_deref(), Some("picocolors"));
    }

    #[test]
    fn a_bun_workspace_without_a_text_lockfile_says_nothing_is_checked() {
        let without = bun_stale(r#""ms@2.1.2":"patches/ms@2.1.2.patch""#, &[]);
        assert_eq!(
            messages(&without),
            [
                "there is no `bun.lock`, so nothing is checked for the `patchedDependencies` entries",
                PNPM_PATCH_LEFT_OVER,
            ]
        );
        // Without a lockfile the report points at the first entry.
        assert_eq!(without[0].0.location.path, Path::new("package.json"));
        assert_eq!(without[0].1, r#""ms@2.1.2""#);
        let binary = bun_stale(r#""ms@2.1.2":"patches/ms@2.1.2.patch""#, &[("bun.lockb", "")]);
        assert_eq!(
            messages(&binary),
            [
                "skif reads `bun.lock` only; the binary `bun.lockb` is not checked for the `patchedDependencies` entries",
                PNPM_PATCH_LEFT_OVER,
            ]
        );
        assert_eq!(binary[0].0.location.path, Path::new("bun.lockb"));
        let broken = bun_stale(
            r#""ms@2.1.2":"patches/ms@2.1.2.patch""#,
            &[("bun.lock", "{ \"packages\": ")],
        );
        assert_eq!(broken.len(), 2, "{broken:#?}");
        assert!(
            broken[0].0.message.starts_with("`bun.lock` cannot be read ("),
            "{}",
            broken[0].0.message
        );
        assert_eq!(broken[0].0.location.path, Path::new("bun.lock"));
        assert_eq!(broken[1].0.message, PNPM_PATCH_LEFT_OVER);
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
        assert_eq!(lint(META.id, &files, &config("")), []);
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
