use std::sync::Arc;

use skif_core::{Category, DependencyKind, Fixability, Related, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_specifier::Specifier;
use skif_workspace::{Catalogs, DependencyEntry};

use super::support::{active_packages, entry_location};

/// Metadata of `incompatible-peer-dependencies`.
pub static META: RuleMeta = RuleMeta {
    id: "incompatible-peer-dependencies",
    category: Category::Manifest,
    default: Some(Severity::Warning),
    fixable: Fixability::None,
    summary: "The version a package installs in `devDependencies` does not satisfy the range it declares in `peerDependencies`.",
};

/// Reports packages whose `devDependencies` version falls outside their own `peerDependencies` range.
#[derive(Debug, Clone, Copy, Default)]
pub struct IncompatiblePeerDependencies;

/// The specifier an entry stands for, following `catalog:` references; `None` for an unresolvable reference.
fn resolve(catalogs: &Catalogs, entry: &DependencyEntry) -> Option<Arc<Specifier>> {
    if entry.parsed.is_catalog() {
        catalogs.resolve(&entry.name, &entry.specifier).map(Specifier::new)
    } else {
        Some(Arc::clone(&entry.parsed))
    }
}

/// Whether the lowest version a dev specifier installs can be told: not for tags, URLs, upper-bounded or compound
/// ranges, or workspace links.
fn has_lowest_version(specifier: &Specifier) -> bool {
    let raw = specifier.get_raw();
    let upper_bounded = raw.starts_with('<') || (raw.starts_with('>') && !raw.starts_with(">="));
    let plain = matches!(
        specifier,
        Specifier::Exact(_)
            | Specifier::Range(_)
            | Specifier::RangeMajor(_)
            | Specifier::RangeMinor(_)
            | Specifier::Major(_)
            | Specifier::Minor(_)
    );
    plain && !upper_bounded
}

/// Whether `dev` installs a version outside `peer`; `false` whenever either side cannot be evaluated.
fn conflicts(peer: &Specifier, dev: &Specifier) -> bool {
    if peer.is_workspace_protocol() || !has_lowest_version(dev) {
        return false;
    }
    match (peer.get_node_range(), dev.get_node_version()) {
        (Some(range), Some(version)) => !range.satisfies(&version),
        _ => false,
    }
}

impl Rule for IncompatiblePeerDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let catalogs = &cx.workspace.catalogs;
        for package in active_packages(cx.workspace) {
            let of_kind =
                move |kind: DependencyKind| package.dependencies.iter().filter(move |entry| entry.kind == kind);
            for peer in of_kind(DependencyKind::PeerDependencies) {
                let Some(dev) = of_kind(DependencyKind::DevDependencies).find(|entry| entry.name == peer.name) else {
                    continue;
                };
                let (Some(peer_spec), Some(dev_spec)) = (resolve(catalogs, peer), resolve(catalogs, dev)) else {
                    continue;
                };
                if !conflicts(&peer_spec, &dev_spec) {
                    continue;
                }
                out.emit(
                    diagnostic(
                        &META,
                        format!(
                            "`{}` is `{}` in devDependencies, which does not satisfy the peer range `{}`",
                            peer.name, dev.specifier, peer.specifier
                        ),
                        entry_location(package, peer),
                    )
                    .package(package.display_name())
                    .dependency(peer.name.clone())
                    .help(format!(
                        "install a version of `{}` that satisfies `{}` in devDependencies, or widen the peer range",
                        peer.name, peer.specifier
                    ))
                    .related(Related {
                        message: format!("`{}` is installed as `{}` here", dev.name, dev.specifier),
                        location: entry_location(package, dev),
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
    use crate::testing::{config, lint, located, root, slice, subjects};

    fn member(peer: &str, dev: &str) -> String {
        format!(r#"{{"name":"a","peerDependencies":{{"react":"{peer}"}},"devDependencies":{{"react":"{dev}"}}}}"#)
    }

    #[test]
    fn range_compatibility_table() {
        let root_json = root("");
        let cases = [
            ("same caret", "^18.0.0", "^18.2.0", false),
            ("exact dev inside", "^18.0.0", "18.3.1", false),
            ("dev below peer", "^18.2.0", "^18.0.0", true),
            ("dev in another major", "^18.0.0", "^17.0.2", true),
            ("or-range satisfied", "^17.0.0 || ^18.0.0", "^18.1.0", false),
            ("or-range violated", "^17.0.0 || ^18.0.0", "^16.0.0", true),
            ("star peer", "*", "^1.0.0", false),
            ("major shorthand dev", "^18.0.0", "18", false),
            ("minor shorthand dev violating", "^18.2.0", "18.1", true),
            ("upper-bounded dev is not judged", "^18.0.0", "<19.0.0", false),
            ("tag dev is not judged", "^18.0.0", "next", false),
            ("hyphen peer", "16.0.0 - 18.0.0", "^17.0.0", false),
            ("prerelease dev outside", "^18.0.0", "19.0.0-rc.1", true),
            ("workspace dev is not judged", "^18.0.0", "workspace:*", false),
        ];
        for (name, peer, dev, expected) in cases {
            let json = member(peer, dev);
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", json.as_str()),
            ];
            assert_eq!(!lint(META.id, &files, &config("")).is_empty(), expected, "{name}");
        }
    }

    #[test]
    fn requires_the_same_name_in_both_sections() {
        let root_json = root("");
        let json = r#"{"name":"a","peerDependencies":{"react":"^18"},"devDependencies":{"vue":"^1"}}"#;
        let files = [("package.json", root_json.as_str()), ("packages/a/package.json", json)];
        assert_eq!(lint(META.id, &files, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn reports_on_the_peer_entry_and_relates_the_dev_entry() {
        let root_json = root("");
        let json = member("^18.2.0", "^17.0.0");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", json.as_str()),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(subjects(&diagnostics), [("a".to_owned(), Some("react".to_owned()))]);
        assert_eq!(located(&files, &diagnostics[0]), r#""react":"^18.2.0""#);
        let related = &diagnostics[0].related[0];
        assert_eq!(
            slice(&files, "packages/a/package.json", related.location.span.unwrap()),
            r#""react":"^17.0.0""#
        );
        assert_eq!(diagnostics[0].fixes, [] as [skif_core::Fix; 0]);
    }

    #[test]
    fn the_root_is_checked_too() {
        let root_json = root(r#","peerDependencies":{"react":"^18.2.0"},"devDependencies":{"react":"^17.0.0"}"#);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", r#"{"name":"a"}"#),
        ];
        assert_eq!(lint(META.id, &files, &config("")).len(), 1);
    }

    #[test]
    fn catalog_references_are_resolved_on_either_side() {
        let root_json = root(r#","catalog":{"react":"^17.0.0"}"#);
        let cases = [
            ("dev via catalog violates", member("^18.0.0", "catalog:"), true),
            ("peer via catalog violates", member("catalog:", "^18.0.0"), true),
            ("satisfied via catalog", member("^17.0.0", "catalog:"), false),
            (
                "missing catalog entry is not judged",
                member("^18.0.0", "catalog:missing"),
                false,
            ),
        ];
        for (name, json, expected) in cases {
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", json.as_str()),
            ];
            assert_eq!(!lint(META.id, &files, &config("")).is_empty(), expected, "{name}");
        }
    }

    #[test]
    fn ignored_dependencies_are_suppressed() {
        let root_json = root("");
        let json = member("^18.2.0", "^17.0.0");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", json.as_str()),
        ];
        let ignored = config("[[ignore]]\nrules = [\"incompatible-peer-dependencies\"]\ndependencies = [\"react\"]\n");
        assert_eq!(lint(META.id, &files, &ignored), [] as [skif_core::Diagnostic; 0]);
    }
}
