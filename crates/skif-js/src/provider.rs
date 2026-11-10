//! The JS rules as one [`RuleProvider`].

use std::sync::Arc;

use skif_config::{Config, ConfigError};
use skif_core::{FileSystem, OsFs, RuleMeta};
use skif_js_syntax::ModuleCache;
use skif_lint::{
    Emitter, EnabledRules, LintContext, OptionsError, ProviderError, RuleProvider, parse_options, reject_options,
};
use skif_workspace::map_ordered;

use crate::analysis::{Caches, JsAnalysis};
use crate::config_check;
use crate::git_index::IndexCache;
use crate::input::InputSource;
use crate::rules::{JsRule, rules};
use crate::setup::UnusedExportsOptions;
use crate::shared_modules::SharedModules;

/// Identifies the JS provider in errors.
pub const PROVIDER_ID: &str = "js";

/// Builds the module graph once per run and evaluates the enabled JS rules on it, in parallel.
///
/// The provider reads source files through the [`FileSystem`] it was given, so it works on disk, in memory or on an
/// editor's buffers. It does nothing when `[js] enabled = false`.
pub struct JsProvider {
    fs: Arc<dyn FileSystem>,
    sources: Vec<Arc<dyn InputSource>>,
    rules: Vec<Box<dyn JsRule>>,
    /// The parsed git indexes, shared by every root this provider analyses.
    indexes: IndexCache,
    /// The cache of parse results this provider's runs parse through: the host's, or the one of a multi-root run.
    modules: Option<SharedModules>,
}

impl JsProvider {
    /// A provider that reads through `fs`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        Self {
            fs,
            sources: Vec::new(),
            rules: rules(),
            indexes: IndexCache::default(),
            modules: None,
        }
    }

    /// A provider that reads the real file system.
    #[must_use]
    pub fn on_disk() -> Self {
        Self::new(Arc::new(OsFs))
    }

    /// Parses source files through `modules`, a cache the host keeps across runs (ADR 0066). The cache changes no
    /// result: its keys cover everything a parse depends on.
    #[must_use]
    pub fn with_module_cache(mut self, modules: Arc<dyn ModuleCache>) -> Self {
        self.modules = Some(SharedModules::host(modules));
        self
    }

    /// Parses source files through `modules`, the cache the linters of one run share (ADR 0101), which decides which
    /// files it holds and whether concurrent lookups of a file wait for one parse. Like any module cache it changes
    /// no result.
    #[must_use]
    pub fn with_shared_modules(mut self, modules: SharedModules) -> Self {
        self.modules = Some(modules);
        self
    }

    /// Adds a source of tool-specific entries, project files, ignores and references (the plugin table).
    #[must_use]
    pub fn with_input_source(mut self, source: Arc<dyn InputSource>) -> Self {
        self.sources.push(source);
        self
    }
}

impl std::fmt::Debug for JsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsProvider")
            .field("input_sources", &self.sources.len())
            .field("module_cache", &self.modules.is_some())
            .finish_non_exhaustive()
    }
}

impl RuleProvider for JsProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn rules(&self) -> Vec<&'static RuleMeta> {
        self.rules.iter().map(|rule| rule.meta()).collect()
    }

    fn validate_options(&self, rule_id: &str, options: &toml::Table) -> Result<(), OptionsError> {
        if rule_id == "unused-exports" {
            parse_options::<UnusedExportsOptions>(options).map(drop)
        } else {
            reject_options(options)
        }
    }

    fn validate_config(&self, config: &Config) -> Result<(), ConfigError> {
        let plugin_ids: Vec<&str> = skif_js_plugins::registry::plugins()
            .iter()
            .map(|plugin| plugin.spec().id)
            .collect();
        config.validate_plugins(&plugin_ids)?;
        config_check::validate(config)
    }

    fn run(&self, cx: &LintContext<'_>, enabled: &EnabledRules, out: &mut Emitter<'_>) -> Result<(), ProviderError> {
        if !cx.config.js.enabled {
            return Ok(());
        }
        let active: Vec<&dyn JsRule> = self
            .rules
            .iter()
            .map(AsRef::as_ref)
            .filter(|rule| enabled.contains(rule.meta().id))
            .collect();
        if active.is_empty() || out.is_cancelled() {
            return Ok(());
        }
        let parser = self.modules.as_ref().map(|modules| modules.parser(&cx.workspace.root));
        let mut analysis = JsAnalysis::build_cached(
            cx.workspace,
            cx.config,
            &self.fs,
            &self.sources,
            cx.sequential,
            &Caches {
                indexes: &self.indexes,
                modules: parser.as_ref(),
                extra_entries: cx.extra_entries,
            },
        );
        analysis.enabled_rules = Some(active.iter().map(|rule| rule.meta().id.to_owned()).collect());
        analysis.config_path = cx.config_path.map(std::path::Path::to_path_buf);
        for package in analysis.incomplete_packages() {
            out.report_incomplete_package(package);
        }
        for file in analysis.other_project_files() {
            out.report_other_project_file(file);
        }
        for (manifest_dir, name) in analysis.dependencies.ancestor_uses() {
            out.report_ancestor_dependency_use(manifest_dir, name);
        }
        let (last, concurrent): (Vec<&dyn JsRule>, Vec<&dyn JsRule>) =
            active.iter().copied().partition(|rule| rule.runs_last());
        let base = &*out;
        let parts: Vec<Emitter<'_>> = map_ordered(&concurrent, cx.sequential, |rule| {
            let mut part = base.fork();
            if !part.is_cancelled() {
                rule.check(&analysis, &mut part);
            }
            part
        });
        // Rules that read what the others recorded run once those have finished.
        let after: Vec<Emitter<'_>> = last
            .iter()
            .map(|rule| {
                let mut part = base.fork();
                if !part.is_cancelled() {
                    rule.check(&analysis, &mut part);
                }
                part
            })
            .collect();
        for part in parts.into_iter().chain(after) {
            out.merge(part);
        }
        Ok(())
    }
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<JsProvider>;
};
