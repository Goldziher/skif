//! The `type-aware` object the JSON document and the SARIF run properties carry: the report's
//! [`TypeAwareSummary`] with each package directory displayed as the other paths of the output are.

use serde::Serialize;
use skif_core::{TypeAwareState, TypeAwareSummary};

use crate::options::RenderOptions;

#[derive(Serialize)]
pub(crate) struct TypeAwareView<'a> {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine: Option<&'a str>,
    packages: Vec<PackageView<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PackageView<'a> {
    dir: String,
    name: &'a str,
    state: TypeAwareState,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    untyped_files: u32,
}

/// `summary` as rendered: package directories relative to the root with `/` separators, `.` for the root package.
pub(crate) fn view<'a>(summary: &'a TypeAwareSummary, options: &RenderOptions) -> TypeAwareView<'a> {
    TypeAwareView {
        enabled: summary.enabled,
        engine: summary.engine.as_deref(),
        packages: summary
            .packages
            .iter()
            .map(|package| {
                let dir = options.display_path(&package.dir);
                PackageView {
                    dir: if dir.is_empty() { ".".to_owned() } else { dir.into_owned() },
                    name: &package.name,
                    state: package.state,
                    reason: package.reason.as_deref(),
                    untyped_files: package.untyped_files,
                }
            })
            .collect(),
    }
}
