use std::ops::Range;
use std::path::{Path, PathBuf};

/// Everything that can go wrong while locating, reading, parsing or validating configuration.
#[derive(Debug, thiserror::Error)]
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
    /// A rule ID in the configuration is not in the registry.
    #[error("unknown rule \"{rule}\"{}", suggestion_hint(.suggestion.as_deref()))]
    UnknownRule { rule: String, suggestion: Option<String> },
}

impl ConfigError {
    pub(crate) fn from_toml(err: &toml::de::Error, source: &str, path: Option<&Path>) -> Self {
        let span = err.span();
        let (line, column) = span.as_ref().map_or((None, None), |range| {
            let (line, column) = line_column(source, range.start);
            (Some(line), Some(column))
        });
        Self::Parse { path: path.map(Path::to_path_buf), message: err.message().to_string(), span, line, column }
    }

    pub(crate) fn invalid(path: Option<&Path>, message: impl Into<String>) -> Self {
        Self::Invalid { path: path.map(Path::to_path_buf), message: message.into() }
    }
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let end = offset.min(source.len());
    let before = source.get(..end).unwrap_or_default();
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |tail| tail.chars().count()) + 1;
    (line, column)
}

fn render(path: Option<&Path>, line: Option<&usize>, column: Option<&usize>, message: &str) -> String {
    let mut out = path.map_or_else(|| "skif config".to_string(), |p| p.display().to_string());
    if let Some(line) = line {
        out.push_str(&format!(":{line}"));
        if let Some(column) = column {
            out.push_str(&format!(":{column}"));
        }
    }
    format!("{out}: {message}")
}

fn suggestion_hint(suggestion: Option<&str>) -> String {
    suggestion.map_or_else(String::new, |s| format!("; did you mean \"{s}\"?"))
}
