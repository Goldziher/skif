use std::collections::BTreeMap;

use skif_config::Config;
use skif_core::{RuleMeta, Severity};
use skif_workspace::Workspace;

/// Read-only inputs shared by every rule.
#[derive(Debug, Clone, Copy)]
pub struct LintContext<'a> {
    pub workspace: &'a Workspace,
    pub config: &'a Config,
    pub enabled: &'a EnabledRules,
}

/// The effective severity of every enabled rule.
///
/// A configured level overrides [`RuleMeta::default`]; `off`, or no default and no level,
/// disables the rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnabledRules {
    severities: BTreeMap<&'static str, Severity>,
}

impl EnabledRules {
    /// Resolves the effective severity of each rule in `metas` against `config`.
    #[must_use]
    pub fn resolve(metas: impl IntoIterator<Item = &'static RuleMeta>, config: &Config) -> Self {
        let severities = metas
            .into_iter()
            .filter_map(|meta| {
                let configured = config.rule(meta.id).and_then(|rule| rule.severity);
                let severity = configured.map_or(meta.default, skif_config::Level::severity)?;
                Some((meta.id, severity))
            })
            .collect();
        Self { severities }
    }

    /// Whether rule `id` runs.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.severities.contains_key(id)
    }

    /// The effective severity of rule `id`, or `None` when it is disabled or unknown.
    #[must_use]
    pub fn severity(&self, id: &str) -> Option<Severity> {
        self.severities.get(id).copied()
    }
}
