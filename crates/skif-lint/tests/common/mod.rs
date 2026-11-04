#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use skif_config::Config;
use skif_core::{Category, Fixability, Location, RuleMeta, Severity};
use skif_lint::{LintContext, Rule, diagnostic};
use skif_workspace::{MemoryFs, Workspace};

pub fn leak_meta(id: &str, default: Option<Severity>) -> &'static RuleMeta {
    let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
    Box::leak(Box::new(RuleMeta::new(
        id,
        Category::Manifest,
        default,
        Fixability::None,
        "test rule",
    )))
}

pub fn config(toml: &str) -> Config {
    Config::from_toml_str(toml).unwrap()
}

/// Root `root` with members `a` and `b`.
pub fn workspace(config: &Config) -> Workspace {
    let fs = MemoryFs::new()
        .with_file(
            "/ws/package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )
        .with_file("/ws/packages/a/package.json", r#"{"name":"a","version":"1.0.0"}"#)
        .with_file("/ws/packages/b/package.json", r#"{"name":"b","version":"1.0.0"}"#);
    Workspace::discover_with_fs(Path::new("/ws"), config, &fs).unwrap()
}

pub fn location() -> Location {
    Location::new(
        PathBuf::from("package.json"),
        None,
    )
}

/// One diagnostic a [`FixedRule`] emits.
#[derive(Clone)]
pub struct Item {
    pub message: String,
    pub package: Option<&'static str>,
    pub dependency: Option<&'static str>,
}

pub fn item(message: &str, package: Option<&'static str>, dependency: Option<&'static str>) -> Item {
    Item {
        message: message.to_owned(),
        package,
        dependency,
    }
}

/// Emits a fixed list of diagnostics.
pub struct FixedRule {
    pub meta: &'static RuleMeta,
    pub items: Vec<Item>,
}

impl Rule for FixedRule {
    fn meta(&self) -> &'static RuleMeta {
        self.meta
    }

    fn check(&self, _cx: &LintContext<'_>, out: &mut skif_lint::Emitter<'_>) {
        for item in &self.items {
            let mut builder = diagnostic(self.meta, item.message.clone(), location());
            if let Some(package) = item.package {
                builder = builder.package(package);
            }
            if let Some(dependency) = item.dependency {
                builder = builder.dependency(dependency);
            }
            out.emit(builder.build());
        }
    }
}
