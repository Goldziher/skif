//! Renderers that turn a [`skif_core::Report`] into human-readable or JSON output.
//!
//! Renderers write to a [`std::io::Write`], never read the environment or detect terminals, and take
//! their configuration from [`RenderOptions`].

mod human;
mod json;
mod options;
mod path;
mod style;

use std::io::{self, Write};

use skif_core::Report;

pub use human::render_human;
pub use json::{SCHEMA_VERSION, render_json, render_json_error};
pub use options::RenderOptions;

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// Grouped, optionally coloured text for terminals.
    Human,
    /// The versioned machine-readable contract.
    Json,
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
    }
}
