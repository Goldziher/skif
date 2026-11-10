//! Rules over `package.json` manifests and the workspace layout.

mod empty_dependencies;
mod incompatible_peer_dependencies;
pub(crate) mod local_path_dependency;
mod member_toolchain_fields;
mod missing_package_manifest;
mod package_json_format;
mod root_package_dependencies;
mod root_package_manager_field;
mod root_package_private_field;
mod stale_patched_dependencies;
pub(crate) mod support;
mod types_in_dependencies;
mod unmatched_workspace_patterns;
mod unsorted_dependencies;

pub use empty_dependencies::EmptyDependencies;
pub use incompatible_peer_dependencies::IncompatiblePeerDependencies;
pub use local_path_dependency::LocalPathDependency;
pub use member_toolchain_fields::{ForbidMode, MemberToolchainFields, MemberToolchainFieldsOptions};
pub use missing_package_manifest::MissingPackageManifest;
pub use package_json_format::PackageJsonFormat;
pub use root_package_dependencies::RootPackageDependencies;
pub use root_package_manager_field::RootPackageManagerField;
pub use root_package_private_field::RootPackagePrivateField;
use skif_lint::{Rule, RulesProvider};
pub use stale_patched_dependencies::{StalePatchedDependencies, StalePatchedDependenciesOptions};
pub use types_in_dependencies::TypesInDependencies;
pub use unmatched_workspace_patterns::UnmatchedWorkspacePatterns;
pub use unsorted_dependencies::UnsortedDependencies;

/// Identifies the manifest provider in errors.
pub const PROVIDER_ID: &str = "manifest";

/// The manifest rules, in the order their diagnostics are merged.
#[must_use]
pub fn rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(EmptyDependencies),
        Box::new(RootPackagePrivateField),
        Box::new(RootPackageManagerField),
        Box::new(RootPackageDependencies),
        Box::new(TypesInDependencies),
        Box::new(MemberToolchainFields),
        Box::new(IncompatiblePeerDependencies),
        Box::new(UnsortedDependencies),
        Box::new(UnmatchedWorkspacePatterns),
        Box::new(MissingPackageManifest),
        Box::new(LocalPathDependency),
        Box::new(PackageJsonFormat),
        Box::new(StalePatchedDependencies),
    ]
}

/// The provider of the manifest rules, with id `"manifest"`.
#[must_use]
pub fn manifest_provider() -> RulesProvider {
    RulesProvider::new(PROVIDER_ID, rules())
}
