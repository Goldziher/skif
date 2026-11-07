use std::collections::BTreeSet;
use std::path::Path;

use skif_core::{
    Category, Fix, FixSafety, Fixability, LineIndex, Location, PathSeg, RuleMeta, Severity, Span, diagnostic,
};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::Workspace;

use super::names::{CatalogName, referenced};
use super::references::override_references;
use super::support::key;
use super::target::{existing_file, remove_entry};

/// Metadata of `unused-catalog-entries`.
pub static META: RuleMeta = RuleMeta::new(
    "unused-catalog-entries",
    Category::Catalog,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A catalog entry that no manifest references; reported only when `[catalog] report-unused-entries` is `true`.",
);

/// Reports catalog entries nobody uses, when `[catalog] report-unused-entries` is on.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnusedCatalogEntries;

impl Rule for UnusedCatalogEntries {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        if !cx.config.catalog.report_unused_entries || !cx.enabled.contains(META.id) {
            return;
        }
        let workspace = cx.workspace;
        let catalogs = &workspace.catalogs;
        let Some(source) = catalogs.source() else {
            return;
        };
        let overrides = override_references(workspace);
        let used: BTreeSet<(CatalogName, &str)> = workspace
            .all_packages()
            .flat_map(|package| &package.dependencies)
            .filter_map(|entry| Some((referenced(&entry.specifier)?, entry.name.as_str())))
            .chain(
                overrides
                    .iter()
                    .filter_map(|instance| Some((referenced(instance.raw())?, instance.name.as_str()))),
            )
            .collect();
        let default = std::iter::once((CatalogName::Default, catalogs.default_catalog()));
        let named = catalogs
            .named()
            .iter()
            .map(|(name, entries)| (CatalogName::Named(name.clone()), entries));
        for (catalog, entries) in default.chain(named) {
            for (name, version) in entries
                .iter()
                .filter(|(name, _)| !used.contains(&(catalog.clone(), name.as_str())))
            {
                let mut builder = diagnostic(
                    &META,
                    format!("{} defines `{name}` but no package references it", catalog.display()),
                    Location::new(source.to_path_buf(), entry_span(workspace, source, &catalog, name)),
                )
                .dependency(name.clone())
                .specifier(version.clone())
                .help("remove the entry or reference it with `catalog:`");
                if source == workspace.root_package().manifest.path.as_path() {
                    builder = builder.package_id(workspace.root_package().id);
                }
                if let Some(file) = existing_file(workspace, &catalog) {
                    builder = builder.fix(Fix::new(
                        format!("Remove `{name}` from {}", catalog.display()),
                        FixSafety::Unsafe,
                        vec![remove_entry(workspace, &file, &catalog, name)],
                    ));
                }
                out.emit(builder.build());
            }
        }
    }
}

fn entry_span(workspace: &Workspace, source: &Path, catalog: &CatalogName, dependency: &str) -> Option<Span> {
    if source == workspace.root_package().manifest.path.as_path() {
        let section: Vec<PathSeg> = match catalog {
            CatalogName::Default => vec![key("catalog")],
            CatalogName::Named(name) => vec![key("catalogs"), key(name)],
        };
        let nested: Vec<PathSeg> = std::iter::once(key("workspaces")).chain(section.clone()).collect();
        return [section, nested].into_iter().find_map(|mut pointer| {
            pointer.push(key(dependency));
            workspace.root_package().manifest.field_span(&pointer)
        });
    }
    let yaml = [&workspace.pnpm_workspace, &workspace.yarnrc]
        .into_iter()
        .flatten()
        .find(|yaml| yaml.path == source)?;
    yaml_key_span(&yaml.source, &yaml.line_index, catalog, dependency)
}

/// The key and the byte length of the token that spells it, quotes included.
fn key_token(line: &str) -> Option<(&str, usize)> {
    if let Some(quote) = line.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        let close = line[1..].find(quote)? + 1;
        return line[close + 1..].starts_with(':').then(|| (&line[1..close], close + 1));
    }
    let name = line[..line.find(':')?].trim_end();
    Some((name, name.len()))
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Finds the key of `dependency` inside the block-style `catalog:` or `catalogs:` mapping of a YAML document.
fn yaml_key_span(source: &str, index: &LineIndex, catalog: &CatalogName, dependency: &str) -> Option<Span> {
    let mut top = None;
    let mut group = None;
    let mut group_indent = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\r', '\n']);
        let trimmed = content.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(['#', '-']) {
            continue;
        }
        let Some((name, length)) = key_token(trimmed) else {
            continue;
        };
        let indent = content.len() - trimmed.len();
        if indent == 0 {
            top = Some(name);
            group = None;
            group_indent = None;
            continue;
        }
        let in_catalog = match (catalog, top) {
            (CatalogName::Default, Some("catalog")) => true,
            (CatalogName::Named(wanted), Some("catalogs")) => {
                if indent == *group_indent.get_or_insert(indent) {
                    group = Some(name);
                    false
                } else {
                    group == Some(wanted.as_str())
                }
            }
            _ => false,
        };
        if in_catalog && name == dependency {
            let begin = start + indent;
            return Some(index.span(to_u32(begin), to_u32(begin + length)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "packages:\n  - \"packages/*\"\ncatalog:\n  \"react\": \"^18.3.1\"\n  left-pad: ^1.3.0\ncatalogs:\n  legacy:\n    \"lodash\": \"^4.17.21\"\n  other:\n    lodash: \"^3.0.0\"\n";

    fn text(catalog: &CatalogName, dependency: &str) -> Option<&'static str> {
        let index = LineIndex::new(YAML);
        let span = yaml_key_span(YAML, &index, catalog, dependency)?;
        Some(&YAML[span.start as usize..span.end as usize])
    }

    #[test]
    fn finds_keys_in_the_right_catalog() {
        let legacy = CatalogName::Named("legacy".to_owned());
        let other = CatalogName::Named("other".to_owned());
        assert_eq!(text(&CatalogName::Default, "react"), Some("\"react\""));
        assert_eq!(text(&CatalogName::Default, "left-pad"), Some("left-pad"));
        assert_eq!(text(&legacy, "lodash"), Some("\"lodash\""));
        assert_eq!(text(&other, "lodash"), Some("lodash"));
        assert_eq!(text(&CatalogName::Default, "lodash"), None);
        assert_eq!(text(&legacy, "react"), None);
    }

    #[test]
    fn overrides_reference_catalog_entries() {
        use crate::testing::{config, lint};

        let root = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","pnpm":{"overrides":{"lodash@<4.17.21":"catalog:legacy","@scope/pkg":"catalog:"}},"resolutions":{"a>glob":"catalog:","**/lodash-es":"catalog:","parent/**/semver@>=7":"catalog:"}}"#;
        let yaml = "packages:\n  - \"packages/*\"\ncatalog:\n  \"@scope/pkg\": ^1.0.0\n  react: ^18.0.0\n  glob: ^10.0.0\n  zod: ^3.0.0\n  lodash-es: ^4.0.0\n  semver: ^7.0.0\ncatalogs:\n  legacy:\n    lodash: ^4.17.21\noverrides:\n  zod: \"catalog:\"\n";
        let files = [
            ("package.json", root),
            ("pnpm-workspace.yaml", yaml),
            ("packages/a/package.json", r#"{"name":"a","private":true}"#),
        ];
        let config = config("[catalog]\nreport-unused-entries = true\n");
        let unused: Vec<String> = lint(META.id, &files, &config)
            .into_iter()
            .filter_map(|diagnostic| diagnostic.dependency)
            .collect();
        assert_eq!(unused, ["react"]);
    }
}
