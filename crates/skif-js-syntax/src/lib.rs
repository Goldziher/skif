//! The oxc boundary of skif: one JavaScript or TypeScript file in, skif-owned facts out.
//!
//! [`parse_module`] returns a [`ModuleInfo`] (imports, exports, in-file references, removal ranges) and
//! [`extract_config_object`] statically evaluates the object a tool config file exports. No oxc type appears in the
//! public API, every public type is `Send + Sync`, and an oxc allocator lives only for the duration of one call.
//!
//! Supported inputs are `.js .mjs .cjs .jsx .ts .mts .cts .tsx` and `.d.ts` files, plus the `<script>` blocks (and
//! Astro frontmatter) of `.vue`, `.svelte` and `.astro` files. Other extensions, including `.mdx`, yield
//! [`SyntaxError::Unsupported`].

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

use std::collections::HashSet;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast_visit::Visit;
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::SourceType;
use skif_core::LineIndex;

pub use config::{ConfigValue, extract_config_object};
pub use error::SyntaxError;
pub use model::{
    Export, ExportKind, Import, ImportKind, ImportedKind, ImportedName, Member, ModuleInfo, ParseIssue, ParseOptions,
    Removal, RemovalKind, coalesce_ranges,
};

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
        "vue" | "svelte" | "astro" => {
            let mut info = ModuleInfo::default();
            for block in sfc::script_blocks(source, extension == "astro") {
                let name = match (block.typescript, block.jsx) {
                    (true, true) => "block.tsx",
                    (true, false) => "block.ts",
                    (false, true) => "block.jsx",
                    (false, false) => "block.js",
                };
                let source_type = source_type_for(Path::new(name)).ok_or_else(|| unsupported(&extension))?;
                let base = u32::try_from(block.start).unwrap_or(u32::MAX);
                let part = parse_block(&source[block.start..block.end], base, source_type, &lines, *options);
                info.merge(part);
            }
            Ok(info)
        }
        "mdx" => Err(unsupported(&extension)),
        _ => {
            let source_type = source_type_for(path).ok_or_else(|| unsupported(&extension))?;
            Ok(parse_block(source, 0, source_type, &lines, *options))
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

fn parse_block(src: &str, base: u32, source_type: SourceType, lines: &LineIndex, options: ParseOptions) -> ModuleInfo {
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
        members::track(&ret.program, &mut info);
    }
    if options.in_file_references {
        mark_in_file_references(&ret.program, &collected.export_clause_spans, &mut info);
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
