//! Renderers that turn a [`skif_core::Report`] into human-readable, machine-readable or Markdown output.
//!
//! Renderers write to a [`std::io::Write`], never read the environment or detect terminals, and take
//! their configuration from [`RenderOptions`].

#![warn(missing_docs)]

mod codeclimate;
mod codeowners;
mod compact;
mod github;
mod human;
mod json;
mod markdown;
mod options;
mod path;
mod sarif;
mod style;
mod text;

use std::io::{self, Write};

use skif_core::Report;

pub use codeclimate::render_codeclimate;
pub use codeowners::{CODEOWNERS_LOCATIONS, CodeOwners, render_codeowners};
pub use compact::render_compact;
pub use github::render_github;
pub use human::render_human;
pub use json::{
    ErrorInfo, SCHEMA_VERSION, render_json, render_json_error, render_json_error_info, render_json_error_info_with,
    render_json_with,
};
pub use markdown::{render_disclosure, render_markdown};
pub use options::RenderOptions;
pub use sarif::render_sarif;
pub use text::{escape_controls, escape_multiline};

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
    /// Code Climate JSON for GitLab Code Quality.
    Codeclimate,
    /// One `path:line:col rule message` line per diagnostic.
    Compact,
    /// Markdown tables per rule, for pull request comments.
    Markdown,
    /// Markdown tables per rule, each folded in a `<details>` element.
    Disclosure,
    /// Compact lines grouped by CODEOWNERS owner.
    Codeowners,
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
        Format::Codeclimate => render_codeclimate(report, options, out),
        Format::Compact => render_compact(report, options, out),
        Format::Markdown => render_markdown(report, options, out),
        Format::Disclosure => render_disclosure(report, options, out),
        Format::Codeowners => render_codeowners(report, options, out),
    }
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<RenderOptions>;
    let _ = assert_send_sync::<ErrorInfo<'static>>;
    let _ = assert_send_sync::<Format>;
    let _ = assert_send_sync::<CodeOwners>;
};
