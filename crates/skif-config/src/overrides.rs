use indexmap::IndexMap;

use crate::rules::Level;

/// The CLI layer: typed command-line flags applied over a loaded configuration.
///
/// `ignore_packages` replaces `[workspace] ignore-packages` when non-empty (arrays replace);
/// `ignore_dependencies` appends an `[[ignore]]` entry covering every rule; `ignore_rules` sets the rules to `off`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigOverrides {
    pub rules: IndexMap<String, Level>,
    pub ignore_rules: Vec<String>,
    pub ignore_packages: Vec<String>,
    pub ignore_dependencies: Vec<String>,
    pub fail_on_warnings: Option<bool>,
    pub production: Option<bool>,
    pub no_js: bool,
}
