//! `Vercel`: the programmatic project config is an entry file.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `vercel` plugin.
pub struct Vercel;

static SPEC: PluginSpec = PluginSpec {
    id: "vercel",
    enablers: &[Enabler::Dependency("@vercel/config")],
    entry: &["vercel.{js,mjs,cjs,ts,mts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Vercel {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
