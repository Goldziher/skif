//! The JS rules as one [`RuleProvider`].

use std::sync::Arc;

use rayon::prelude::*;
use skif_core::{FileSystem, OsFs, RuleMeta};
use skif_lint::{Emitter, EnabledRules, LintContext, ProviderError, RuleProvider};

use crate::analysis::JsAnalysis;
use crate::input::InputSource;
use crate::rules::{JsRule, rules};

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
}

impl JsProvider {
    /// A provider that reads through `fs`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        Self {
            fs,
            sources: Vec::new(),
            rules: rules(),
        }
    }

    /// A provider that reads the real file system.
    #[must_use]
    pub fn on_disk() -> Self {
        Self::new(Arc::new(OsFs))
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
        let analysis = JsAnalysis::build(cx.workspace, cx.config, &self.fs, &self.sources);
        let base = &*out;
        let parts: Vec<Emitter<'_>> = active
            .par_iter()
            .map(|rule| {
                let mut part = base.fork();
                if !part.is_cancelled() {
                    rule.check(&analysis, &mut part);
                }
                part
            })
            .collect();
        for part in parts {
            out.merge(part);
        }
        Ok(())
    }
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<JsProvider>;
};
