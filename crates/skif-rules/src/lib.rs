//! Built-in rules for skif.
//!
//! Rules only detect: they report every finding to the [`Emitter`](skif_lint::Emitter), which owns severity and
//! suppression. Fixes are data ([`skif_core::Fix`]) addressed relative to the workspace root; applying them is the
//! host's job (see `skif-edit`).
//!
//! [`manifest`] holds the rules over `package.json` manifests and the workspace layout. [`policy`] holds the version
//! group and semver group policies. [`toolchain`] holds the rules over the version files, Dockerfiles, CI workflows
//! and shell scripts beside the manifests, which read those files through a [`skif_core::FileSystem`] and so come in
//! their own provider, [`toolchain_provider`]. [`providers`] returns every other built-in [`RuleProvider`], ready to
//! register in a [`skif_lint::Registry`]. [`update`] plans `skif update`: newer published versions, through a
//! registry the host supplies.

#![warn(missing_docs)]

pub mod manifest;
pub mod policy;
pub mod toolchain;
pub mod update;

pub use manifest::manifest_provider;
pub use policy::{catalog_provider, policy_provider};
pub use toolchain::toolchain_provider;
use skif_lint::RuleProvider;

/// Every built-in rule provider that reads only the workspace; [`toolchain_provider`] is the one that reads more files.
#[must_use]
pub fn providers() -> Vec<Box<dyn RuleProvider>> {
    vec![
        Box::new(manifest_provider()),
        Box::new(policy_provider()),
        Box::new(catalog_provider()),
    ]
}

#[cfg(test)]
mod testing;

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<skif_lint::RulesProvider>;
};
