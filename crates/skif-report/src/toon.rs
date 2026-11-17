//! The TOON renderer: the JSON document in Token-Oriented Object Notation (ADR 0012).
//!
//! TOON carries the same data model as JSON with fewer tokens: no quotes around plain keys and strings, indentation
//! instead of braces, and arrays of uniform objects as one header plus a row per element. It is for LLM agents and
//! other consumers that pay per token; the document is exactly the JSON one, so `--format json` stays the contract
//! and this is its encoding.

use std::io::{self, Write};

use serde_json::{Map, Value};
use skif_core::Report;

use crate::json::{ErrorInfo, render_json_error_info_with, render_json_with};
use crate::options::RenderOptions;

/// Renders `report` as the JSON document of [`crate::render_json`], encoded as TOON.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_toon(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    render_toon_with(report, options, Map::new(), out)
}

/// Renders `report` like [`render_toon`] with the top-level `extra` members of [`crate::render_json_with`].
///
/// # Errors
///
/// The errors of [`crate::render_json_with`] (an `extra` member that shadows a reserved key), and any error produced
/// by writing to `out`.
#[allow(
    clippy::needless_pass_by_value,
    reason = "mirrors `render_json_with`, which takes `extra` by value"
)]
pub fn render_toon_with(
    report: &Report,
    options: &RenderOptions,
    extra: Map<String, Value>,
    out: &mut dyn Write,
) -> io::Result<()> {
    let mut json = Vec::new();
    render_json_with(report, options, extra, &mut json)?;
    encode(&json, out)
}

/// Renders the error document of [`crate::render_json_error_info_with`], encoded as TOON.
///
/// # Errors
///
/// The errors of [`crate::render_json_error_info_with`], and any error produced by writing to `out`.
pub fn render_toon_error_info_with(
    info: &ErrorInfo<'_>,
    extra: Map<String, Value>,
    out: &mut dyn Write,
) -> io::Result<()> {
    let mut json = Vec::new();
    render_json_error_info_with(info, extra, &mut json)?;
    encode(&json, out)
}

/// Re-encodes a JSON document as TOON, ending with a newline as the JSON renderings do.
fn encode(json: &[u8], out: &mut dyn Write) -> io::Result<()> {
    let document: Value = serde_json::from_slice(json)?;
    let mut text = serde_toon::to_string(&document).map_err(io::Error::other)?;
    text.push('\n');
    out.write_all(text.as_bytes())
}
