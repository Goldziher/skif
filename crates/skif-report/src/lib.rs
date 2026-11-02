//! Renderers that turn a [`skif_core::Report`] into human-readable or JSON output.
//!
//! Renderers write to a [`std::io::Write`], never read the environment or detect terminals, and take
//! their configuration from [`RenderOptions`].

mod github;
mod human;
mod json;
mod options;
mod path;
mod sarif;
mod style;

use std::io::{self, Write};

use skif_core::Report;

pub use github::render_github;
pub use human::render_human;
pub use json::{ErrorInfo, SCHEMA_VERSION, render_json, render_json_error, render_json_error_info, render_json_with};
pub use options::RenderOptions;
pub use sarif::render_sarif;

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// Grouped, optionally coloured text for terminals.
    Human,
    /// The versioned machine-readable contract.
    Json,
    /// SARIF 2.1.0 for code scanning.
    Sarif,
    /// GitHub Actions workflow-command annotations.
    Github,
}

/// Renders `report` in `format` to `out`.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render(report: &Report, format: Format, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    match format {
        Format::Human => render_human(report, options, out),
        Format::Json => render_json(report, options, out),
        Format::Sarif => render_sarif(report, options, out),
        Format::Github => render_github(report, options, out),
    }
}
