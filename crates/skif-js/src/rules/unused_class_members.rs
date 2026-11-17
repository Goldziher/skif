use skif_core::{Category, Fix, FixSafety, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::{delete_range, module_location};
use crate::analysis::JsAnalysis;
use crate::class_members::{
    ClassFacts, MemberUsage, NameReads, declaration_name, entry_exposed, exemption, judged_classes, judges_classes_of,
};

/// Metadata of `unused-class-members`.
pub static META: RuleMeta = RuleMeta::new(
    "unused-class-members",
    Category::Js,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A member of an exported class is never read.",
);

/// Reports members of used exported classes that no module reads (ADR 0009).
///
/// The class must be used by another module; an unused class is reported by `unused-exports`. A member is used when
/// a module reads a property of its name, also through an element access whose key is a constant (ADR 0009); the
/// declarations decide which members are not judged at all, among them a field that exists only for the type checker
/// ([`crate::class_members::MemberExemption`]). The fix deletes the member ([`skif_js_syntax::ClassMember::removal`]).
pub struct UnusedClassMembers;

impl JsRule for UnusedClassMembers {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let judged: Vec<_> = analysis
            .graph
            .modules()
            .iter()
            .filter(|module| module.info.exports.iter().any(|export| export.class.is_some()))
            .filter(|module| judges_classes_of(analysis, module))
            .collect();
        if judged.is_empty() {
            return;
        }
        let usage = NameReads::of(&analysis.graph);
        let exposed = entry_exposed(analysis);
        for module in judged {
            for (export, shape) in judged_classes(analysis, module) {
                let facts = ClassFacts::of(analysis, module, export, shape, &exposed);
                for member in &shape.members {
                    if exemption(analysis, module, shape, &facts, member).is_some()
                        || usage.is_used(module, export, member)
                    {
                        continue;
                    }
                    let class = declaration_name(export);
                    let mut builder = diagnostic(
                        &META,
                        format!("class member `{class}.{}` is never read", member.name),
                        module_location(module, Some(member.span)),
                    )
                    .help("remove the member, tag it `@public`, or list it in `ignore-members`");
                    if let Some(package) = analysis.workspace.package(module.owner) {
                        builder = builder.package(package.display_name());
                    }
                    if let Some((start, end)) = member.removal {
                        builder = builder.fix(Fix::new(
                            format!("Remove the class member `{}`", member.name),
                            FixSafety::Unsafe,
                            vec![delete_range(module, start, end)],
                        ));
                    }
                    out.emit(builder.build());
                }
            }
        }
    }
}
