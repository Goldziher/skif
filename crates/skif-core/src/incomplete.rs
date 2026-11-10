//! Packages whose entry files are not all known, so their files and dependencies are not judged unused.
//!
//! The JS analysis records these; a [`Report`](crate::Report) carries them so a consumer can tell "nothing is unused
//! here" from "this was not judged".

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A package whose entry files are not all known, with every reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct IncompletePackage {
    /// The package name, or its directory when it has none.
    pub name: String,
    /// The package directory, relative to the workspace root (empty for the root package).
    pub dir: PathBuf,
    /// Why its entries are incomplete; never empty.
    pub reasons: Vec<IncompleteEntries>,
}

impl IncompletePackage {
    /// A package named `name` in `dir` whose entries are incomplete for `reasons`.
    #[must_use]
    pub const fn new(name: String, dir: PathBuf, reasons: Vec<IncompleteEntries>) -> Self {
        Self { name, dir, reasons }
    }
}

/// Why the entry files of a package are not all known, so its files and dependencies are not judged unused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub struct IncompleteEntries {
    /// What could not be read.
    pub cause: IncompleteCause,
    /// The tool whose configuration could not be read, when it is a tool's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// The file that could not be read: absolute where the analysis records it, relative to the workspace root with
    /// `/` separators in a report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

/// What made the entry files of a package incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum IncompleteCause {
    /// A tool configuration does not parse.
    UnparsableConfig,
    /// A tool configuration evaluates to a value that is not known statically.
    UnknownConfig,
    /// A tool configuration spreads or merges in a value that is not known statically, such as an imported base
    /// config.
    UnknownSpread,
    /// The keys a tool reads entry files from are computed in a way that cannot be evaluated statically.
    ComputedEntries,
    /// `package.json` publishes build output or bin files none of which maps back to a source file.
    UnmappedPublished,
    /// The package has no source file, entry or script of its own (an install hook aside): its manifest lists what
    /// code elsewhere installs it for.
    NoEntries,
    /// An input source reported the entries incomplete without a reason.
    Unspecified,
}

impl IncompleteCause {
    /// A sentence fragment for terminal output.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::UnparsableConfig => "the configuration does not parse",
            Self::UnknownConfig => "the configuration evaluates to a value that is not known statically",
            Self::UnknownSpread => {
                "the configuration spreads or merges in a value that is not known statically (an imported base config)"
            }
            Self::ComputedEntries => "the keys the tool reads entry files from are computed",
            Self::UnmappedPublished => {
                "package.json publishes build output or bin files none of which maps back to a source file"
            }
            Self::NoEntries => "the package has no source file, entry or script of its own",
            Self::Unspecified => "an input source reported its entries incomplete",
        }
    }
}
