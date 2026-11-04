use std::ops::Range;
use std::path::{Path, PathBuf};

use skif_core::LineIndex;

/// Everything that can go wrong while locating, reading, parsing or validating configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The TOML is malformed, has an unknown key, or a value of the wrong type.
    #[error("{}", render(.path.as_deref(), .line.as_ref(), .column.as_ref(), .message))]
    Parse {
        path: Option<PathBuf>,
        message: String,
        /// Byte range in the source, when the parser reports one.
        span: Option<Range<usize>>,
        /// 1-based line of the start of `span`.
        line: Option<usize>,
        /// 1-based column (in characters) of the start of `span`.
        column: Option<usize>,
    },
    /// The file parsed but its contents are inconsistent.
    #[error("{}", render(.path.as_deref(), None, None, .message))]
    Invalid { path: Option<PathBuf>, message: String },
    /// The configuration file could not be read.
    #[error("cannot read {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Both `skif.toml` and `.skif.toml` exist.
    #[error("both {} and {} exist; keep only one", .first.display(), .second.display())]
    Ambiguous { first: PathBuf, second: PathBuf },
    /// A configuration file extends itself, directly or through other files.
    #[error("configuration files extend each other in a cycle: {}", join_paths(.chain))]
    ExtendsCycle { chain: Vec<PathBuf> },
    /// A key is used in a place that cannot hold it: a root-scoped setting in an override, or `root` in a file that
    /// is not discovered.
    #[error("{}", render(.path.as_deref(), .line.as_ref(), .column.as_ref(), &scope_message(.key, .hint)))]
    Scope {
        /// The offending key, as a dotted path.
        key: String,
        path: Option<PathBuf>,
        /// 1-based line of the key.
        line: Option<usize>,
        /// 1-based column (in characters) of the key.
        column: Option<usize>,
        /// Where the setting belongs.
        hint: String,
    },
    /// A rule ID in the configuration is not in the registry.
    #[error("unknown rule \"{rule}\"{}", suggestion_hint(.suggestion.as_deref()))]
    UnknownRule { rule: String, suggestion: Option<String> },
    /// The options configured for a rule are invalid, as judged by the rule that owns them.
    #[error("invalid options for rule \"{rule}\"{}: {message}", key_hint(.key.as_deref()))]
    InvalidRuleOptions {
        rule: String,
        /// The offending option key, when known.
        key: Option<String>,
        message: String,
    },
}

impl ConfigError {
    pub(crate) fn from_toml(err: &toml::de::Error, source: &str, path: Option<&Path>) -> Self {
        let span = err.span();
        let (line, column) = span.as_ref().map_or((None, None), |range| {
            let (line, column) = line_column(source, range.start);
            (Some(line), Some(column))
        });
        Self::Parse {
            path: path.map(Path::to_path_buf),
            message: err.message().to_string(),
            span,
            line,
            column,
        }
    }

    /// A copy of this error; `std::io::Error` is not `Clone`, so an I/O error keeps its kind and message.
    pub(crate) fn duplicate(&self) -> Self {
        match self {
            Self::Parse {
                path,
                message,
                span,
                line,
                column,
            } => Self::Parse {
                path: path.clone(),
                message: message.clone(),
                span: span.clone(),
                line: *line,
                column: *column,
            },
            Self::Invalid { path, message } => Self::Invalid {
                path: path.clone(),
                message: message.clone(),
            },
            Self::Io { path, source } => Self::Io {
                path: path.clone(),
                source: std::io::Error::new(source.kind(), source.to_string()),
            },
            Self::Ambiguous { first, second } => Self::Ambiguous {
                first: first.clone(),
                second: second.clone(),
            },
            Self::ExtendsCycle { chain } => Self::ExtendsCycle { chain: chain.clone() },
            Self::Scope {
                key,
                path,
                line,
                column,
                hint,
            } => Self::Scope {
                key: key.clone(),
                path: path.clone(),
                line: *line,
                column: *column,
                hint: hint.clone(),
            },
            Self::UnknownRule { rule, suggestion } => Self::UnknownRule {
                rule: rule.clone(),
                suggestion: suggestion.clone(),
            },
            Self::InvalidRuleOptions { rule, key, message } => Self::InvalidRuleOptions {
                rule: rule.clone(),
                key: key.clone(),
                message: message.clone(),
            },
        }
    }

    pub(crate) fn scope(
        key: impl Into<String>,
        path: Option<&Path>,
        position: Option<(usize, usize)>,
        hint: impl Into<String>,
    ) -> Self {
        Self::Scope {
            key: key.into(),
            path: path.map(Path::to_path_buf),
            line: position.map(|(line, _)| line),
            column: position.map(|(_, column)| column),
            hint: hint.into(),
        }
    }

    pub(crate) fn invalid(path: Option<&Path>, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }
}

fn scope_message(key: &str, hint: &str) -> String {
    format!("`{key}` is not allowed here; {hint}")
}

fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(" -> ")
}

pub(crate) fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let offset = u32::try_from(offset.min(source.len())).unwrap_or(u32::MAX);
    let (line, column) = LineIndex::new(source).line_col(offset);
    (line as usize, column as usize)
}

fn render(path: Option<&Path>, line: Option<&usize>, column: Option<&usize>, message: &str) -> String {
    let location = path.map_or_else(|| "skif config".to_string(), |p| p.display().to_string());
    match (line, column) {
        (Some(line), Some(column)) => format!("{location}:{line}:{column}: {message}"),
        (Some(line), None) => format!("{location}:{line}: {message}"),
        _ => format!("{location}: {message}"),
    }
}

fn suggestion_hint(suggestion: Option<&str>) -> String {
    suggestion.map_or_else(String::new, |s| format!("; did you mean \"{s}\"?"))
}

fn key_hint(key: Option<&str>) -> String {
    key.map_or_else(String::new, |key| format!(" (option `{key}`)"))
}
