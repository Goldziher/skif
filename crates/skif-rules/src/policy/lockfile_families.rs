//! `unsynced-dependency-family` over the lockfile: a family with `scope = "importer"` or `scope = "lockfile"` must
//! resolve to one version in what each importer reaches or across `pnpm-lock.yaml`; with an `anchor`, each member may
//! resolve once per major of the anchor.

use std::collections::{BTreeMap, BTreeSet};

use skif_config::FamilyScope;
use skif_core::{Location, diagnostic};
use skif_lint::{Emitter, LintContext};
use skif_specifier::Specifier;
use skif_workspace::PnpmLockfile;

use super::families::{Family, META};
use super::specifiers::{floor_version, major};
use crate::lockfile::{self, importer_package, snapshot_location};

/// Reports the members of the lockfile-scoped `families` that resolve to more versions than the family allows.
pub(super) fn emit(cx: &LintContext<'_>, families: &[Family], out: &mut Emitter<'_>) {
    if families.is_empty() {
        return;
    }
    let loaded = lockfile::load(cx);
    let lock = match loaded.get(
        cx,
        &META,
        "the dependency families with a lockfile scope",
        Location::new(cx.config_file(), None),
    ) {
        Ok(lock) => lock,
        Err(diagnostic) => {
            out.emit(diagnostic);
            return;
        }
    };
    for family in families {
        match family.scope {
            FamilyScope::Importer => {
                for importer in lock.importers.keys() {
                    let resolutions = lock.resolutions(importer);
                    check(cx, lock, family, Some(importer.as_str()), resolutions.keys(), out);
                }
            }
            FamilyScope::Lockfile => check(cx, lock, family, None, lock.snapshots.keys().map(String::as_str), out),
            _ => {}
        }
    }
}

/// Checks `family` over the snapshots `keys`, the resolutions of `importer` or, without one, of the whole lockfile.
fn check<'l>(
    cx: &LintContext<'_>,
    lock: &'l PnpmLockfile,
    family: &Family,
    importer: Option<&str>,
    keys: impl Iterator<Item = &'l str>,
    out: &mut Emitter<'_>,
) {
    // Each release once, by name and version: peer variants of one release resolve the same code.
    let mut releases: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    let mut anchor_majors = BTreeSet::new();
    for key in keys {
        let snapshot = &lock.snapshots[key];
        if family.anchor.as_deref() == Some(snapshot.name.as_str())
            && let Some(anchor_major) = major(&Specifier::new(&snapshot.version))
        {
            anchor_majors.insert(anchor_major);
        }
        if family.members.matches(&snapshot.name, false) {
            releases
                .entry((snapshot.name.as_str(), snapshot.version.as_str()))
                .or_insert(key);
        }
    }
    // Without an anchor the whole family shares one version; with one, each member shares a version per major.
    let mut buckets: BTreeMap<(Option<&str>, u64), Vec<_>> = BTreeMap::new();
    for (&(name, version), &key) in &releases {
        let specifier = Specifier::new(version);
        let (Some(release_major), Some(rank)) = (major(&specifier), floor_version(&specifier)) else {
            continue;
        };
        let bucket = if family.anchor.is_some() {
            (Some(name), release_major)
        } else {
            (None, 0)
        };
        buckets.entry(bucket).or_default().push((name, version, key, rank));
    }
    let scope = importer.map_or_else(String::new, |importer| format!(" in importer `{importer}`"));
    let package = importer.and_then(|importer| importer_package(cx.workspace, importer));
    let report = |out: &mut Emitter<'_>, name: &str, version: &str, key: &str, message: String| {
        let mut builder = diagnostic(&META, message, snapshot_location(lock, key))
            .dependency(name)
            .specifier(version)
            .help(format!(
                "update what pulls in `{name}` so the `{}` family resolves as one, or align it with an `overrides` entry",
                family.name
            ));
        if let Some(package) = package {
            builder = builder.package(package.display_name()).package_id(package.id);
        }
        out.emit(builder.build());
    };
    for ((_, bucket_major), members) in &buckets {
        let anchor = family.anchor.as_deref();
        if let Some(anchor) = anchor
            && !anchor_majors.is_empty()
            && !anchor_majors.contains(bucket_major)
        {
            let majors = anchor_majors.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
            for &(name, version, key, _) in members {
                report(
                    out,
                    name,
                    version,
                    key,
                    format!(
                        "`{name}@{version}` of the `{}` family matches no major `{anchor}` resolves to ({majors}){scope}",
                        family.name
                    ),
                );
            }
            continue;
        }
        // The first of the highest releases leads, so the leader does not depend on how many share its version.
        let Some(&(leader, leader_version, _, ref highest)) = members.iter().rev().max_by(|a, b| a.3.cmp(&b.3)) else {
            continue;
        };
        for &(name, version, key, ref rank) in members {
            if rank >= highest {
                continue;
            }
            let message = match anchor {
                Some(anchor) => format!(
                    "`{name}@{version}` resolves next to `{leader}@{leader_version}`; the `{}` family allows one \
                     version per major of `{anchor}`{scope}",
                    family.name
                ),
                None => format!(
                    "`{name}@{version}` resolves next to `{leader}@{leader_version}` of the `{}` family{scope}",
                    family.name
                ),
            };
            report(out, name, version, key, message);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use skif_core::Diagnostic;

    use super::*;
    use crate::testing::{config, lint, located, root};

    /// `app` reaches `playwright-core@1.44.0` through `@playwright/test@1.45.0`; `docs` reaches `1.45.0` only.
    const PLAYWRIGHT: &str = "lockfileVersion: '9.0'

importers:

  .: {}

  packages/app:
    devDependencies:
      '@playwright/test':
        specifier: 1.45.0
        version: 1.45.0

  packages/docs:
    devDependencies:
      playwright-core:
        specifier: 1.45.0
        version: 1.45.0

packages:

  '@playwright/test@1.45.0':
    resolution: {integrity: sha512-test}

  playwright-core@1.44.0:
    resolution: {integrity: sha512-core-44}

  playwright-core@1.45.0:
    resolution: {integrity: sha512-core-45}

  playwright@1.45.0:
    resolution: {integrity: sha512-playwright}

snapshots:

  '@playwright/test@1.45.0':
    dependencies:
      playwright: 1.45.0

  playwright-core@1.44.0: {}

  playwright-core@1.45.0: {}

  playwright@1.45.0:
    dependencies:
      playwright-core: 1.44.0
";

    /// Two React majors with their types: one extra `@types/react` 18 and an `@types/react` 16 no React matches.
    const REACT_TYPES: &str = "lockfileVersion: '9.0'

importers:

  .: {}

packages:

  '@types/react@16.9.0':
    resolution: {integrity: sha512-t16}

  '@types/react@17.0.80':
    resolution: {integrity: sha512-t17}

  '@types/react@18.3.1':
    resolution: {integrity: sha512-t181}

  '@types/react@18.3.3':
    resolution: {integrity: sha512-t183}

  react@17.0.2:
    resolution: {integrity: sha512-r17}

  react@18.3.1:
    resolution: {integrity: sha512-r18}

snapshots:

  '@types/react@16.9.0': {}

  '@types/react@17.0.80': {}

  '@types/react@18.3.1': {}

  '@types/react@18.3.3': {}

  react@17.0.2: {}

  react@18.3.1: {}
";

    const APP: &str = r#"{"name":"app","version":"1.0.0","devDependencies":{"@playwright/test":"1.45.0","playwright-core":"1.44.0"}}"#;
    const DOCS: &str = r#"{"name":"docs","version":"1.0.0","devDependencies":{"playwright-core":"1.45.0"}}"#;

    fn family(scope: &str) -> String {
        format!(
            "[[dependency-families]]\nname = \"playwright\"\npackages = [\"playwright\", \"playwright-core\", \"@playwright/test\"]\nscope = \"{scope}\"\n"
        )
    }

    fn unsynced(lock: Option<&str>, toml: &str) -> Vec<(Diagnostic, String)> {
        let root_json = root("");
        let mut files = vec![
            ("package.json", root_json.as_str()),
            ("packages/app/package.json", APP),
            ("packages/docs/package.json", DOCS),
        ];
        if let Some(lock) = lock {
            files.push(("pnpm-lock.yaml", lock));
        }
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

    #[test]
    fn a_family_member_on_another_version_in_an_importer_is_reported_and_the_manifests_are_left_alone() {
        let found = unsynced(Some(PLAYWRIGHT), &family("importer"));
        assert_eq!(found.len(), 1, "{found:#?}");
        let (diagnostic, text) = &found[0];
        assert_eq!(
            diagnostic.message,
            "`playwright-core@1.44.0` resolves next to `@playwright/test@1.45.0` of the `playwright` family in \
             importer `packages/app`"
        );
        assert_eq!(diagnostic.location.path, Path::new("pnpm-lock.yaml"));
        assert_eq!(text, "playwright-core@1.44.0: {}");
        assert_eq!(diagnostic.package.as_deref(), Some("app"));
        assert_eq!(diagnostic.dependency.as_deref(), Some("playwright-core"));
        assert_eq!(diagnostic.specifier.as_deref(), Some("1.44.0"));
        // The manifest of `app` declares the same mismatch; in the manifests scope that is what is reported.
        let declared = unsynced(Some(PLAYWRIGHT), &family("manifests"));
        assert_eq!(declared.len(), 1, "{declared:#?}");
        assert_eq!(declared[0].0.location.path, Path::new("packages/app/package.json"));
    }

    #[test]
    fn the_lockfile_scope_compares_every_resolution_of_the_family() {
        let found = unsynced(Some(PLAYWRIGHT), &family("lockfile"));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(
            found[0].0.message,
            "`playwright-core@1.44.0` resolves next to `@playwright/test@1.45.0` of the `playwright` family"
        );
        assert_eq!(found[0].0.package, None);
    }

    #[test]
    fn with_an_anchor_each_member_resolves_once_per_major_of_the_anchor() {
        let toml = "[[dependency-families]]\nname = \"react-types\"\npackages = [\"@types/react\"]\nscope = \"lockfile\"\nanchor = \"react\"\n";
        let found = unsynced(Some(REACT_TYPES), toml);
        let messages: Vec<&str> = found.iter().map(|(diagnostic, _)| diagnostic.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "`@types/react@16.9.0` of the `react-types` family matches no major `react` resolves to (17, 18)",
                "`@types/react@18.3.1` resolves next to `@types/react@18.3.3`; the `react-types` family allows one \
                 version per major of `react`",
            ]
        );
        assert_eq!(found[1].1, "'@types/react@18.3.1': {}");
    }

    #[test]
    fn a_family_with_a_lockfile_scope_and_no_lockfile_is_reported() {
        let found = unsynced(None, &family("importer"));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(
            found[0].0.message,
            "there is no `pnpm-lock.yaml`, so nothing is checked for the dependency families with a lockfile scope"
        );
        let broken = unsynced(Some("lockfileVersion: '9.0'\nsnapshots: {a\n"), &family("lockfile"));
        assert_eq!(broken.len(), 1, "{broken:#?}");
        assert!(broken[0].0.message.starts_with("`pnpm-lock.yaml` cannot be read ("), "{}", broken[0].0.message);
    }
}
