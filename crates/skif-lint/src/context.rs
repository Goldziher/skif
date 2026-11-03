use std::collections::BTreeMap;
use std::path::Path;

use skif_config::Config;
use skif_core::{Diagnostic, RuleMeta, Severity};
use skif_workspace::Workspace;

/// Read-only inputs shared by every rule.
#[derive(Debug, Clone, Copy)]
pub struct LintContext<'a> {
    pub workspace: &'a Workspace,
    pub config: &'a Config,
    pub enabled: &'a EnabledRules,
    /// The configuration file `config` was loaded from, relative to the workspace root; `None` when the host
    /// does not know it (for example a configuration built in memory).
    pub config_path: Option<&'a Path>,
    /// Providers run their work on the calling thread and must not use rayon.
    pub sequential: bool,
}

impl LintContext<'_> {
    /// Name of the configuration file used by diagnostics about the configuration itself, relative to the
    /// workspace root: [`config_path`](Self::config_path), or [`DEFAULT_CONFIG_FILE`] when unknown.
    #[must_use]
    pub fn config_file(&self) -> &Path {
        self.config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_FILE))
    }
}

/// The configuration file name assumed when the host does not say where the configuration came from.
pub const DEFAULT_CONFIG_FILE: &str = "skif.toml";

/// The effective severity of every enabled rule.
///
/// A configured level overrides [`RuleMeta::default`]; `off`, or no default and no level,
/// disables the rule. A rule that sets [`Diagnostic::default_severity`] replaces its [`RuleMeta::default`] for that
/// finding, but never a configured level; see [`effective`](Self::effective).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnabledRules {
    /// The effective severity, and whether the configuration set it (as opposed to the rule's default).
    severities: BTreeMap<&'static str, (Severity, bool)>,
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
                Some((meta.id, (severity, configured.is_some())))
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
        self.severities.get(id).map(|(severity, _)| *severity)
    }

    /// Whether the configuration sets the level of rule `id`, as opposed to the level coming from the rule's own
    /// default.
    #[must_use]
    pub fn is_configured(&self, id: &str) -> bool {
        self.severities.get(id).is_some_and(|(_, configured)| *configured)
    }

    /// The severity `diagnostic` is reported with, or `None` when its rule is disabled: the configured level when
    /// there is one, otherwise the severity the rule proposed for the finding, otherwise the rule's default.
    #[must_use]
    pub fn effective(&self, diagnostic: &Diagnostic) -> Option<Severity> {
        let (severity, configured) = *self.severities.get(diagnostic.rule.as_str())?;
        Some(if configured {
            severity
        } else {
            diagnostic.default_severity.unwrap_or(severity)
        })
    }
}
