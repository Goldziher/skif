//! `banned-dependency` over the lockfile: the `scope = "lockfile"` banned groups judge every package `pnpm-lock.yaml`
//! resolves, transitive ones included, and report it with the shortest chain an importer pulls it in through.

use std::collections::BTreeMap;

use skif_config::{GroupScope, VersionGroup, VersionPolicy};
use skif_core::{Location, diagnostic};
use skif_lint::{Emitter, LintContext};
use skif_workspace::Package;

use super::state::BANNED_DEPENDENCY;
use crate::lockfile::{self, display_key, importer_package, snapshot_location};

/// A banned resolution and the shortest way an importer reaches it.
struct Finding<'l> {
    group: usize,
    key: &'l str,
    importer: &'l str,
    chain: Vec<&'l str>,
}

/// Reports the resolved packages the lockfile-scoped banned groups forbid, once per `name@version`.
pub(super) fn emit(cx: &LintContext<'_>, out: &mut Emitter<'_>) {
    let groups: Vec<(usize, &VersionGroup)> = cx
        .config
        .version_groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.scope == GroupScope::Lockfile && group.policy == VersionPolicy::Banned)
        .collect();
    if groups.is_empty() {
        return;
    }
    let loaded = lockfile::load(cx);
    let lock = match loaded.get(
        cx,
        &BANNED_DEPENDENCY,
        "the `scope = \"lockfile\"` version groups",
        Location::new(cx.config_file(), None),
    ) {
        Ok(lock) => lock,
        Err(diagnostic) => {
            out.emit(diagnostic);
            return;
        }
    };
    let admits = |group: &VersionGroup, name: &str, package: Option<&Package>| {
        (group.dependencies.is_empty() || group.dependencies.matches(name, false))
            && (group.packages.is_empty() || package.is_some_and(|package| package.is_selected_by(&group.packages)))
    };
    // By `name@version`: peer variants of one release are one finding.
    let mut findings: BTreeMap<String, Finding<'_>> = BTreeMap::new();
    for importer in lock.importers.keys() {
        let package = importer_package(cx.workspace, importer);
        let resolutions = lock.resolutions(importer);
        for key in resolutions.keys() {
            let name = lock.snapshots[key].name.as_str();
            let Some(&(group, _)) = groups.iter().find(|entry| admits(entry.1, name, package)) else {
                continue;
            };
            let Some(chain) = resolutions.chain(key) else {
                continue;
            };
            let shorter = findings
                .get(&display_key(key))
                .is_none_or(|found| chain.len() < found.chain.len());
            if shorter {
                findings.insert(
                    display_key(key),
                    Finding {
                        group,
                        key,
                        importer,
                        chain,
                    },
                );
            }
        }
    }
    for (release, finding) in findings {
        let group = &cx.config.version_groups[finding.group];
        let by = group.label.as_deref().map_or_else(
            || "a banned version group".to_owned(),
            |label| format!("the banned version group `{label}`"),
        );
        let path = finding
            .chain
            .iter()
            .map(|key| display_key(key))
            .collect::<Vec<_>>()
            .join(" > ");
        let name = &lock.snapshots[finding.key].name;
        let version = &lock.snapshots[finding.key].version;
        let package = importer_package(cx.workspace, finding.importer);
        let mut builder = diagnostic(
            &BANNED_DEPENDENCY,
            format!(
                "`{release}` is resolved although {by} forbids it; importer `{}` pulls it in through `{path}`",
                finding.importer
            ),
            snapshot_location(lock, finding.key),
        )
        .dependency(name.clone())
        .specifier(version.clone())
        .detail("IsBanned")
        .help(format!(
            "remove the dependency that pulls in `{name}`, or keep it out with an `overrides` entry"
        ));
        if let Some(package) = package {
            builder = builder.package(package.display_name()).package_id(package.id);
        }
        out.emit(builder.build());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use skif_core::Diagnostic;

    use super::*;
    use crate::testing::{config, lint, located, root};

    const APP: &str = r#"{"name":"app","version":"1.0.0","dependencies":{"mid":"^2.0.0"}}"#;

    const BAN_BAD: &str = "[[version-groups]]\ndependencies = [\"bad\"]\npolicy = \"banned\"\nscope = \"lockfile\"\n";

    /// `app` depends on `mid`, which depends on the banned `bad`.
    const LOCK: &str = "lockfileVersion: '9.0'

importers:

  .: {}

  packages/app:
    dependencies:
      mid:
        specifier: ^2.0.0
        version: 2.0.0

packages:

  bad@1.0.0:
    resolution: {integrity: sha512-bad}

  mid@2.0.0:
    resolution: {integrity: sha512-mid}

snapshots:

  bad@1.0.0: {}

  mid@2.0.0:
    dependencies:
      bad: 1.0.0
";

    /// The same graph while `overrides` replaces `bad` with `good`.
    const OVERRIDDEN: &str = "lockfileVersion: '9.0'

overrides:
  bad: npm:good@1.0.0

importers:

  .: {}

  packages/app:
    dependencies:
      mid:
        specifier: ^2.0.0
        version: 2.0.0

packages:

  good@1.0.0:
    resolution: {integrity: sha512-good}

  mid@2.0.0:
    resolution: {integrity: sha512-mid}

snapshots:

  good@1.0.0: {}

  mid@2.0.0:
    dependencies:
      bad: good@1.0.0
";

    fn banned(lock: Option<&str>, toml: &str) -> (Vec<Diagnostic>, String) {
        let root_json = root("");
        let mut files = vec![("package.json", root_json.as_str()), ("packages/app/package.json", APP)];
        if let Some(lock) = lock {
            files.push(("pnpm-lock.yaml", lock));
        }
        let diagnostics = lint(BANNED_DEPENDENCY.id, &files, &config(toml));
        // The text the first located diagnostic points at.
        let text = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.location.span.is_some())
            .map(|diagnostic| located(&files, diagnostic).to_owned())
            .unwrap_or_default();
        (diagnostics, text)
    }

    #[test]
    fn a_transitive_banned_package_is_reported_with_the_chain_that_pulls_it_in() {
        let (diagnostics, text) = banned(Some(LOCK), BAN_BAD);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        let diagnostic = &diagnostics[0];
        assert_eq!(
            diagnostic.message,
            "`bad@1.0.0` is resolved although a banned version group forbids it; importer `packages/app` pulls it in \
             through `mid@2.0.0 > bad@1.0.0`"
        );
        assert_eq!(diagnostic.location.path, Path::new("pnpm-lock.yaml"));
        assert_eq!(text, "bad@1.0.0: {}");
        assert_eq!(diagnostic.package.as_deref(), Some("app"));
        assert_eq!(diagnostic.dependency.as_deref(), Some("bad"));
        assert_eq!(diagnostic.specifier.as_deref(), Some("1.0.0"));
        assert_eq!(diagnostic.detail.as_deref(), Some("IsBanned"));
    }

    #[test]
    fn removing_the_override_that_kept_a_banned_package_out_is_reported() {
        let (kept_out, _) = banned(Some(OVERRIDDEN), BAN_BAD);
        assert!(kept_out.is_empty(), "{kept_out:#?}");
        let (reintroduced, _) = banned(Some(LOCK), BAN_BAD);
        assert_eq!(
            reintroduced.iter().map(|d| d.dependency.as_deref()).collect::<Vec<_>>(),
            [Some("bad")]
        );
    }

    #[test]
    fn a_lockfile_group_judges_direct_dependencies_in_the_lockfile_and_claims_no_manifest_entry() {
        let toml = "[[version-groups]]\nlabel = \"no mid\"\ndependencies = [\"mid\"]\npolicy = \"banned\"\nscope = \"lockfile\"\n";
        let (diagnostics, _) = banned(Some(LOCK), toml);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            "`mid@2.0.0` is resolved although the banned version group `no mid` forbids it; importer `packages/app` \
             pulls it in through `mid@2.0.0`"
        );
        assert_eq!(diagnostics[0].location.path, Path::new("pnpm-lock.yaml"));
    }

    #[test]
    fn the_packages_selector_chooses_the_importers() {
        let toml = format!("{BAN_BAD}packages = [\"other\"]\n");
        let (diagnostics, _) = banned(Some(LOCK), &toml);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let toml = format!("{BAN_BAD}packages = [\"app\"]\n");
        assert_eq!(banned(Some(LOCK), &toml).0.len(), 1);
    }

    #[test]
    fn a_missing_or_foreign_lockfile_is_reported_instead_of_passing() {
        let (missing, _) = banned(None, BAN_BAD);
        assert_eq!(missing.len(), 1, "{missing:#?}");
        assert_eq!(
            missing[0].message,
            "there is no `pnpm-lock.yaml`, so nothing is checked for the `scope = \"lockfile\"` version groups"
        );
        assert_eq!(missing[0].location.path, Path::new("skif.toml"));
        let root_json = root("");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/app/package.json", APP),
            ("yarn.lock", "# yarn lockfile v1\n"),
        ];
        let foreign = lint(BANNED_DEPENDENCY.id, &files, &config(BAN_BAD));
        assert_eq!(foreign.len(), 1, "{foreign:#?}");
        assert_eq!(
            foreign[0].message,
            "skif reads `pnpm-lock.yaml` only; `yarn.lock` is not checked for the `scope = \"lockfile\"` version groups"
        );
        assert_eq!(foreign[0].location.path, Path::new("yarn.lock"));
    }

    #[test]
    fn a_lockfile_that_does_not_parse_is_reported() {
        let (diagnostics, _) = banned(Some("lockfileVersion: '9.0'\nimporters: [\n"), BAN_BAD);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(
            diagnostics[0].message.starts_with("`pnpm-lock.yaml` cannot be read ("),
            "{}",
            diagnostics[0].message
        );
        assert!(
            diagnostics[0]
                .message
                .ends_with("so nothing is checked for the `scope = \"lockfile\"` version groups")
        );
        assert_eq!(diagnostics[0].location.path, Path::new("pnpm-lock.yaml"));
        let (old, _) = banned(Some("lockfileVersion: '6.0'\n"), BAN_BAD);
        assert!(
            old[0].message.contains("lockfileVersion 6.0 is not supported"),
            "{}",
            old[0].message
        );
    }

    #[test]
    fn without_a_lockfile_group_the_lockfile_is_not_read() {
        // A lockfile that does not parse would be reported if anything read it.
        let (diagnostics, _) = banned(
            Some("not: [valid\n"),
            "[[version-groups]]\ndependencies = [\"bad\"]\npolicy = \"banned\"\n",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }
}
