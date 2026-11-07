//! The oxc boundary of skif: one JavaScript or TypeScript file in, skif-owned facts out.
//!
//! [`parse_module`] returns a [`ModuleInfo`] (imports, exports, in-file references, removal ranges) and
//! [`extract_config_object`] statically evaluates the object a tool config file exports. No oxc type appears in the
//! public API, every public type is `Send + Sync`, and an oxc allocator lives only for the duration of one call.
//!
//! Supported inputs are `.js .mjs .cjs .jsx .ts .mts .cts .tsx` and `.d.ts` files, plus the `<script>` blocks (and
//! Astro frontmatter), the `<style>` imports and the markup identifiers of `.vue`, `.svelte` and `.astro` files:
//! component tags and the expressions of the template count as uses of the imports they name. Stylesheets go
//! through [`parse_stylesheet`]. Other extensions, including `.mdx`, yield [`SyntaxError::Unsupported`].

#![warn(missing_docs)]

mod collect;
mod comments;
mod config;
mod error;
mod finder;
mod jsdoc;
mod members;
mod model;
mod semantic;
mod sfc;
mod signature_refs;
mod style;
mod template;

use std::collections::HashSet;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast_visit::Visit;
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::SourceType;
use skif_core::LineIndex;

pub use config::{
    ConfigValue, ImportedCall, TEMPLATE_SUBSTITUTION, extract_config_object, extract_constructor_options,
    extract_imported_calls,
};
pub use error::SyntaxError;
pub use model::{
    Export, ExportKind, Import, ImportKind, ImportedKind, ImportedName, Member, ModuleInfo, ParseIssue, ParseOptions,
    Removal, RemovalKind, coalesce_ranges,
};
pub use style::{is_stylesheet, parse_stylesheet};

/// Parses `source` (the content of `path`) into a [`ModuleInfo`]. The extension of `path` selects the language.
///
/// # Errors
///
/// [`SyntaxError::Unsupported`] for a file type that is neither a script nor a supported component format.
///
/// Syntax errors are not errors of this function: the parser's recovered program is analysed and the problems are
/// reported in [`ModuleInfo::parse_diagnostics`]. When the parser cannot recover at all, the returned info is empty
/// and [`ModuleInfo::fatal`] is set.
pub fn parse_module(path: &Path, source: &str, options: &ParseOptions) -> Result<ModuleInfo, SyntaxError> {
    let lines = LineIndex::new(source);
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "vue" | "svelte" | "astro" => parse_component(&extension, source, &lines, *options),
        "mdx" => Err(unsupported(&extension)),
        _ => {
            let source_type = source_type_for(path).ok_or_else(|| unsupported(&extension))?;
            Ok(parse_block(source, 0, source_type, &lines, *options, None))
        }
    }
}

/// Source type for `path`. JavaScript is parsed with JSX enabled because real projects write JSX in `.js` files.
fn source_type_for(path: &Path) -> Option<SourceType> {
    let source_type = SourceType::from_path(path).ok()?;
    Some(if source_type.is_javascript() {
        source_type.with_jsx(true)
    } else {
        source_type
    })
}

fn unsupported(extension: &str) -> SyntaxError {
    SyntaxError::Unsupported {
        extension: extension.to_string(),
    }
}

/// Parses one block of code. With `uses`, also records how every name is used in the block.
fn parse_block(
    src: &str,
    base: u32,
    source_type: SourceType,
    lines: &LineIndex,
    options: ParseOptions,
    uses: Option<&mut members::Uses>,
) -> ModuleInfo {
    let allocator = Allocator::default();
    let parser_options = OxcParseOptions {
        preserve_parens: false,
        allow_return_outside_function: true,
        ..OxcParseOptions::default()
    };
    let ret = Parser::new(&allocator, src, source_type)
        .with_options(parser_options)
        .parse();
    let parse_diagnostics: Vec<ParseIssue> = ret
        .diagnostics
        .errors()
        .map(|diagnostic| {
            let (offset, len) = diagnostic
                .labels
                .as_slice()
                .first()
                .map_or((0, 0), |l| (l.offset(), l.len()));
            ParseIssue {
                message: diagnostic.message.to_string(),
                span: lines.span(
                    base.saturating_add(offset),
                    base.saturating_add(offset).saturating_add(len),
                ),
            }
        })
        .collect();
    if ret.fatal_error {
        return ModuleInfo {
            parse_diagnostics,
            fatal: true,
            ..ModuleInfo::default()
        };
    }

    let ctx = collect::Ctx { src, base, lines };
    let mut info = ModuleInfo {
        parse_diagnostics,
        ..ModuleInfo::default()
    };
    let collected = collect::collect(&ctx, &ret.program, &mut info);
    finder::Finder::new(&ctx, &mut info).visit_program(&ret.program);
    info.imports.sort_by_key(|i| i.span.start);
    if options.member_accesses {
        let excluded: HashSet<(u32, u32)> = collected.reexport_spans.iter().copied().collect();
        members::track(&ret.program, &mut info, &excluded);
        if let Some(uses) = uses {
            *uses = members::uses(&ret.program);
        }
    }
    if options.in_file_references {
        mark_in_file_references(&ret.program, &collected.export_clause_spans, &mut info);
        signature_refs::mark(&ret.program, &mut info);
        members::mark_local_reads(&ret.program, &mut info);
    }
    info.directives = ret
        .program
        .directives
        .iter()
        .map(|d| d.directive.as_str().to_string())
        .collect();
    comments::collect(&ctx, &ret.program, &mut info);
    info
}

/// Parses a `.vue`, `.svelte` or `.astro` file: its script blocks (and Astro frontmatter), the imports of its style
/// blocks and `src` attributes, and the identifiers its markup uses.
///
/// Spans of every fact are offsets into `source`. Script blocks are analysed as one module: an import of one block
/// that another block or the markup reads counts as used there.
fn parse_component(
    extension: &str,
    source: &str,
    lines: &LineIndex,
    options: ParseOptions,
) -> Result<ModuleInfo, SyntaxError> {
    let blocks = sfc::blocks(source, extension == "astro");
    let mut parts = Vec::new();
    let mut all_uses = Vec::new();
    for block in blocks.scripts.iter().filter(|block| block.parse) {
        let name = match (block.typescript, block.jsx) {
            (true, true) => "block.tsx",
            (true, false) => "block.ts",
            (false, true) => "block.jsx",
            (false, false) => "block.js",
        };
        let source_type = source_type_for(Path::new(name)).ok_or_else(|| unsupported(extension))?;
        let base = u32::try_from(block.start).unwrap_or(u32::MAX);
        let mut uses = members::Uses::new();
        let part = parse_block(
            &source[block.start..block.end],
            base,
            source_type,
            lines,
            options,
            Some(&mut uses),
        );
        parts.push(part);
        all_uses.push(uses);
    }
    if options.member_accesses {
        for (index, part) in parts.iter_mut().enumerate() {
            let mut others = members::Uses::new();
            for (other, uses) in all_uses.iter().enumerate() {
                if other != index {
                    members::merge_uses(&mut others, uses);
                }
            }
            members::credit(part, &others);
        }
    }
    let mut info = ModuleInfo::default();
    for part in parts {
        info.merge(part);
    }
    add_style_and_source_imports(&mut info, &blocks, source, lines);
    if options.member_accesses {
        let flavor = match extension {
            "vue" => template::Flavor::Vue,
            "svelte" => template::Flavor::Svelte,
            _ => template::Flavor::Astro,
        };
        let mut uses = members::Uses::new();
        for (start, end) in blocks.markup(source) {
            template::scan(&source[start..end], flavor, &mut uses);
        }
        members::credit(&mut info, &uses);
    }
    Ok(info)
}

/// Adds the imports of `<style>` blocks and of `src="…"` attributes of `<script>` and `<style>` tags.
fn add_style_and_source_imports(info: &mut ModuleInfo, blocks: &sfc::Blocks, source: &str, lines: &LineIndex) {
    for style in &blocks.styles {
        let path = format!("block.{}", style.extension);
        let base = u32::try_from(style.start).unwrap_or(u32::MAX);
        for mut import in parse_stylesheet(Path::new(&path), &source[style.start..style.end]).imports {
            import.span = lines.span(
                base.saturating_add(import.span.start),
                base.saturating_add(import.span.end),
            );
            info.imports.push(import);
        }
    }
    for reference in &blocks.sources {
        info.imports.push(Import {
            specifier: reference.specifier.clone(),
            kind: ImportKind::SideEffect,
            names: Vec::new(),
            span: lines.span(
                u32::try_from(reference.start).unwrap_or(u32::MAX),
                u32::try_from(reference.end).unwrap_or(u32::MAX),
            ),
        });
    }
    info.imports.sort_by_key(|import| import.span.start);
}

fn mark_in_file_references(program: &oxc_ast::ast::Program<'_>, excluded: &[oxc_span::Span], info: &mut ModuleInfo) {
    let wanted: HashSet<String> = info
        .exports
        .iter()
        .filter(|e| {
            !matches!(
                e.kind,
                ExportKind::ReExport | ExportKind::StarReExport | ExportKind::Default
            )
        })
        .filter_map(|e| e.local_name.clone())
        .collect();
    let referenced = semantic::referenced_locals(program, &wanted, excluded);
    for export in &mut info.exports {
        let used = export.local_name.as_ref().is_some_and(|l| referenced.contains(l))
            && !matches!(
                export.kind,
                ExportKind::ReExport | ExportKind::StarReExport | ExportKind::Default
            );
        export.referenced_in_file = used;
        if used {
            info.in_file_references.push(export.name.clone());
        }
    }
    info.in_file_references.sort();
    info.in_file_references.dedup();
}

/// An oxc string as an owned Rust string. Lone surrogates, which only escaped JS strings can hold,
/// are replaced rather than dropped.
pub(crate) fn js_string(value: oxc_str::JSStr<'_>) -> String {
    value
        .as_str()
        .map_or_else(|| value.display().to_string(), str::to_owned)
}
