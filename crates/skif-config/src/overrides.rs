use std::path::PathBuf;

use indexmap::IndexMap;

use crate::rules::Level;

/// The CLI layer: typed command-line flags applied over a loaded configuration.
///
/// `ignore_packages` replaces `[workspace] ignore-packages` when non-empty (arrays replace);
/// `ignore_dependencies` appends an `[[ignore]]` entry covering every rule; `ignore_rules` sets the rules to `off`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ConfigOverrides {
    /// Rule levels (`--rule ID=LEVEL`).
    pub rules: IndexMap<String, Level>,
    /// Rules to turn off (`--ignore-rule`).
    pub ignore_rules: Vec<String>,
    /// Package names or globs that replace `[workspace] ignore-packages` (`--ignore-package`).
    pub ignore_packages: Vec<String>,
    /// Dependency names that no rule reports (`--ignore-dependency`).
    pub ignore_dependencies: Vec<String>,
    /// Overrides `[lint] fail-on-warnings` (`--fail-on-warnings`).
    pub fail_on_warnings: Option<bool>,
    /// Overrides `[js] production` (`--production`).
    pub production: Option<bool>,
    /// Turns the JS analysis off (`--no-js`).
    pub no_js: bool,
    /// Overrides `[js] type-aware` at the top level and in every workspace that sets it (`--type-aware`,
    /// `--no-type-aware`); `None` leaves the files' choice.
    pub type_aware: Option<bool>,
    /// Use this file (and what it extends) instead of discovering configuration files (`--config`).
    pub config_file: Option<PathBuf>,
    /// Read no configuration file at all: the defaults plus these overrides (`--no-config`).
    pub no_config: bool,
}

impl ConfigOverrides {
    /// Sets the level of each rule in `rules`.
    #[must_use]
    pub fn with_rules(mut self, rules: impl IntoIterator<Item = (String, Level)>) -> Self {
        self.rules = rules.into_iter().collect();
        self
    }

    /// Turns the rules in `ids` off.
    #[must_use]
    pub fn with_ignore_rules(mut self, ids: Vec<String>) -> Self {
        self.ignore_rules = ids;
        self
    }

    /// Replaces `[workspace] ignore-packages` with `patterns`.
    #[must_use]
    pub fn with_ignore_packages(mut self, patterns: Vec<String>) -> Self {
        self.ignore_packages = patterns;
        self
    }

    /// Hides the dependencies named in `names` from every rule.
    #[must_use]
    pub fn with_ignore_dependencies(mut self, names: Vec<String>) -> Self {
        self.ignore_dependencies = names;
        self
    }

    /// Overrides `[lint] fail-on-warnings`; `None` keeps the configured value.
    #[must_use]
    pub const fn with_fail_on_warnings(mut self, value: Option<bool>) -> Self {
        self.fail_on_warnings = value;
        self
    }

    /// Overrides `[js] production`; `None` keeps the configured value.
    #[must_use]
    pub const fn with_production(mut self, value: Option<bool>) -> Self {
        self.production = value;
        self
    }

    /// Turns the JS analysis off when `no_js` is set.
    #[must_use]
    pub const fn with_no_js(mut self, no_js: bool) -> Self {
        self.no_js = no_js;
        self
    }

    /// Turns the type-aware analysis off when `no_type_aware` is set, overriding `[js] type-aware` and every
    /// `[js.workspaces."<dir>"] type-aware`.
    #[must_use]
    pub const fn with_no_type_aware(mut self, no_type_aware: bool) -> Self {
        if no_type_aware {
            self.type_aware = Some(false);
        }
        self
    }

    /// Turns the type-aware analysis on when `type_aware` is set, overriding `[js] type-aware` and every
    /// `[js.workspaces."<dir>"] type-aware`; [`Self::with_no_type_aware`] wins over it, called before or after.
    #[must_use]
    pub const fn with_type_aware(mut self, type_aware: bool) -> Self {
        if type_aware && !matches!(self.type_aware, Some(false)) {
            self.type_aware = Some(true);
        }
        self
    }

    /// Uses `file` (and what it extends) instead of discovering configuration files.
    #[must_use]
    pub fn with_config_file(mut self, file: Option<PathBuf>) -> Self {
        self.config_file = file;
        self
    }

    /// Reads no configuration file when `no_config` is set.
    #[must_use]
    pub const fn with_no_config(mut self, no_config: bool) -> Self {
        self.no_config = no_config;
        self
    }
}
